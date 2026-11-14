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

use super::control::{self, Run};
use super::world::{Verdict, World};
use crate::cost::Cost;
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::{NetId, NodeId};
use crate::search::{Search, SearchOutcome, Source, Target};
use crate::space::{DenseSlots, FastMap, NodeSlots, RoutingSpace, Step};

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
pub(crate) const DIRS: [(i64, i64); 8] = [
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
    /// By cell of a grid, the pads whose boxes, widened by the distance from which routes
    /// move onto their lattices, reach into it, in order.
    pad_index: FastMap<(i64, i64), Vec<u32>>,
    /// The side of the cells of `pad_index`.
    pad_cell: i64,
    /// How far from a pad's box routes move onto its lattice.
    snap: i64,
    /// The origin of the net's own lattice, which its routes are on away from its pads:
    /// the centre of the first pad of its first terminal.
    origin: Point,
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
        let pad_cell = (2 * snap).max(1);
        let mut pad_index: FastMap<(i64, i64), Vec<u32>> = FastMap::default();
        for (i, p) in pads.iter().enumerate() {
            let b = p.bbox.expand(snap);
            for cy in b.min().y.div_euclid(pad_cell)..=b.max().y.div_euclid(pad_cell) {
                for cx in b.min().x.div_euclid(pad_cell)..=b.max().x.div_euclid(pad_cell) {
                    pad_index.entry((cx, cy)).or_default().push(i as u32);
                }
            }
        }
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
        let origin = pads
            .iter()
            .min_by_key(|p| p.terminal)
            .map_or(Point::new(0, 0), |p| p.center);
        NetCtx {
            net,
            origin,
            widths,
            via,
            pads,
            terminals,
            pad_index,
            pad_cell,
            snap,
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
    /// Whether to route what can be reached and give up on the rest, rather than fail:
    /// when the search to the terminals left fails, each is searched from on its own, to
    /// the tree, and those that aren't reached are left out, in [`Tree::missing`].
    pub(crate) partial: bool,
    /// Whether the net's terminals are stubs to its first, a plane, rather than pads to
    /// join in a tree: each search is then for one terminal, the nearest the first of
    /// those left, with a share of the budget, [`PARTIAL_SHARE`]. Searches for every
    /// terminal at once cost a lower bound to each for every node, which a ground net of a
    /// hundred pads can't afford.
    pub(crate) stubs: bool,
}

/// Verdicts of the world on steps of one pitch, by node and direction, which hold for as
/// long as the world and the net's lattice don't change: three bits for each direction,
/// whether it is known and then the verdict. And its verdicts on the net's vias, which are
/// asked about once for every layer.
#[derive(Debug, Default)]
pub(crate) struct StepCache {
    steps: RefCell<FastMap<DNode, u32>>,
    vias: RefCell<FastMap<Point, Verdict>>,
}

impl StepCache {
    /// What is known of the steps from a node, for [`known`](StepCache::known).
    fn bits(&self, from: DNode) -> u32 {
        self.steps.borrow().get(&from).copied().unwrap_or(0)
    }

    /// The verdict on the step in a direction, if `bits` have it.
    fn known(bits: u32, dir: usize) -> Option<Verdict> {
        let bits = bits >> (3 * dir);
        let verdict = match (bits >> 1) & 3 {
            0 => Verdict::Free,
            1 => Verdict::Routed,
            _ => Verdict::Blocked,
        };
        (bits & 1 != 0).then_some(verdict)
    }

    fn set(&self, from: DNode, dir: usize, verdict: Verdict) {
        let code = match verdict {
            Verdict::Free => 0,
            Verdict::Routed => 1,
            Verdict::Blocked => 2,
        };
        *self.steps.borrow_mut().entry(from).or_insert(0) |= (1 | code << 1) << (3 * dir);
    }
}

/// Slots for the nodes of a net's searches, numbered as they are first met and kept from
/// one search to the next: a net's searches go over the same nodes again and again, from a
/// tree that only grows, so a node met before is looked up rather than numbered again, and
/// is in the same slot, where a search finds the bound it had for it
/// ([`Search::reuse_bounds`]).
#[derive(Debug, Default)]
pub(crate) struct KeptSlots {
    slots: FastMap<DNode, usize>,
}

impl NodeSlots<DNode> for KeptSlots {
    fn slot(&mut self, key: DNode) -> usize {
        let next = self.slots.len();
        *self.slots.entry(key).or_insert(next)
    }

    fn get(&self, key: DNode) -> Option<usize> {
        self.slots.get(&key).copied()
    }

    fn clear(&mut self) {}
}

/// The side of the cells that the boxes of a tree's runs are kept by, in pitches, and how
/// many rings of them around a point's the bound on the way to the tree looks into for the
/// nearest run: a tree of a hundred pads has thousands of runs, and a search towards it
/// asks for the bound at every node.
const BOUND_CELL: i64 = 16;
const BOUND_RINGS: i64 = 2;

/// The layer of the node that stands for a net's whole tree, in a search towards it.
const TREE_LAYER: u8 = u8::MAX;

/// The node that stands for a net's whole tree: every node of the tree steps to it at no
/// cost, so that a search from a terminal ends at the first of them it reaches.
const TREE: DNode = DNode {
    x: 0,
    y: 0,
    l: TREE_LAYER,
};

/// The space one net is searched in.
pub(crate) struct DetailSpace<'a> {
    world: &'a World<'a>,
    ctx: &'a NetCtx,
    cache: &'a StepCache,
    params: Params,
    corridor: Option<&'a Corridor>,
    rip: Option<RipUp<'a>>,
    /// Whether other nets' routes may be crossed, if `rip` has a penalty for it.
    cross: bool,
    /// A box within the bounds that tracks are kept to, if any.
    window: Option<Rect>,
    layers: usize,
    near: RefCell<Vec<u32>>,
    /// The net's own vias so far, which others of its vias keep the hole-to-hole
    /// clearance from: the world doesn't have them until the net is committed.
    keep_off: Vec<Point>,
    /// By terminal, whether the net's tree has reached it yet.
    reached: Vec<bool>,
    /// Whether the search is from a terminal towards the tree, [`TREE`], rather than from
    /// the tree to terminals.
    to_tree: bool,
    /// The nodes of the tree.
    tree: FastMap<DNode, ()>,
    /// The tree's nodes by layer and cell of a pitch, for stepping onto it from another
    /// lattice.
    tree_cells: FastMap<(i64, i64, u8), Vec<Point>>,
    /// The boxes of the tree's straight runs, which the bound on the way to it is to.
    tree_boxes: Vec<Rect>,
    /// The boxes by cell of [`BOUND_CELL`] pitches that they reach into, as positions in
    /// `tree_boxes`.
    tree_grid: FastMap<(i64, i64), Vec<u32>>,
    /// A box around the boxes of each path and each terminal added to the tree.
    tree_groups: Vec<Rect>,
    /// The run the search is part of, which hears of every node.
    run: Option<&'a Run<'a>>,
}

impl<'a> DetailSpace<'a> {
    pub(crate) fn new(
        world: &'a World<'a>,
        ctx: &'a NetCtx,
        cache: &'a StepCache,
        params: Params,
        corridor: Option<&'a Corridor>,
        rip: Option<RipUp<'a>>,
    ) -> DetailSpace<'a> {
        DetailSpace {
            world,
            ctx,
            cache,
            params,
            corridor,
            rip,
            cross: true,
            window: None,
            layers: world.rules().layer_count(),
            near: RefCell::new(Vec::new()),
            keep_off: Vec::new(),
            reached: vec![false; ctx.terminals],
            to_tree: false,
            tree: FastMap::default(),
            tree_cells: FastMap::default(),
            tree_boxes: Vec::new(),
            tree_grid: FastMap::default(),
            tree_groups: Vec::new(),
            run: world.run(),
        }
    }

    /// Whether the search may cross other nets' routes.
    fn crossing(&self) -> bool {
        self.cross && self.rip.is_some_and(|r| r.penalty.is_some())
    }

    /// Adds a node to the tree, and returns whether it is new to it.
    fn grow(&mut self, n: DNode) -> bool {
        if self.tree.insert(n, ()).is_some() {
            return false;
        }
        let pitch = self.params.pitch;
        self.tree_cells
            .entry((n.x.div_euclid(pitch), n.y.div_euclid(pitch), n.l))
            .or_default()
            .push(n.point());
        true
    }

    /// Adds a box to those the tree is bounded by.
    fn grow_box(&mut self, b: Rect) {
        let cell = BOUND_CELL * self.params.pitch;
        let i = self.tree_boxes.len() as u32;
        self.tree_boxes.push(b);
        for cy in b.min().y.div_euclid(cell)..=b.max().y.div_euclid(cell) {
            for cx in b.min().x.div_euclid(cell)..=b.max().x.div_euclid(cell) {
                self.tree_grid.entry((cx, cy)).or_default().push(i);
            }
        }
    }

    /// The boxes added since there were `start` of them, as one of the groups that the
    /// bound from afar is to.
    fn group_boxes(&mut self, start: usize) {
        if let Some(around) = self.tree_boxes[start..]
            .iter()
            .copied()
            .reduce(|a, b| a.union(&b))
        {
            self.tree_groups.push(around);
        }
    }

    /// Adds the boxes of a path's straight runs to those the tree is bounded by.
    fn grow_boxes(&mut self, path: &[DNode]) {
        let Some(&first) = path.first() else { return };
        let start = self.tree_boxes.len();
        let mut run = Rect::new(first.point(), first.point());
        let mut heading = None;
        for w in path.windows(2) {
            let (a, b) = (w[0], w[1]);
            let d = ((b.x - a.x).signum(), (b.y - a.y).signum());
            if heading.is_some_and(|h| h != d) {
                self.grow_box(run);
                run = Rect::new(a.point(), a.point());
            }
            heading = Some(d);
            run = run.union(&Rect::new(b.point(), b.point()));
        }
        self.grow_box(run);
        self.group_boxes(start);
    }

    /// A lower bound on the length of track from `p` to the tree, in nanometres: to the
    /// nearest of its runs if one is near, and otherwise to the nearest group of them, or
    /// as far as was looked for a run.
    fn to_tree_bound(&self, p: Point) -> f64 {
        let to = |b: &Rect| {
            let q = Point::new(
                p.x.clamp(b.min().x, b.max().x),
                p.y.clamp(b.min().y, b.max().y),
            );
            octile(p, q)
        };
        let cell = BOUND_CELL * self.params.pitch;
        let (cx, cy) = (p.x.div_euclid(cell), p.y.div_euclid(cell));
        let mut best = f64::INFINITY;
        for r in 0..=BOUND_RINGS {
            for (i, j) in (-r..=r).flat_map(|i| (-r..=r).map(move |j| (i, j))) {
                if i.abs().max(j.abs()) != r {
                    continue;
                }
                for &k in self.tree_grid.get(&(cx + i, cy + j)).into_iter().flatten() {
                    best = best.min(to(&self.tree_boxes[k as usize]));
                }
            }
            // Every run not looked at yet is at least `r` cells away.
            if best <= (r * cell) as f64 {
                return best;
            }
        }
        let looked = (BOUND_RINGS * cell) as f64;
        let far = self
            .tree_groups
            .iter()
            .map(to)
            .fold(f64::INFINITY, f64::min);
        let bound = best.min(far.max(looked));
        if bound.is_finite() { bound } else { 0.0 }
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
        // Two holes of one size are closer than `h2h` when their centres are closer than
        // that and their diameter.
        let (dx, dy) = ((a.x - b.x) as i128, (a.y - b.y) as i128);
        let reach = i128::from(drill) + i128::from(h2h);
        h2h > 0 && dx * dx + dy * dy < reach * reach
    }

    /// Keeps the net's later vias clear of one at `p`.
    fn keep_off(&mut self, p: Point) {
        self.keep_off.push(p);
    }

    /// What a step costs on top of its length, or `None` if it may not be taken.
    fn verdict_cost(&self, verdict: Verdict, history: f64) -> Option<f64> {
        let penalty = self.rip.and_then(|r| r.penalty).filter(|_| self.cross);
        match (verdict, penalty) {
            (Verdict::Free, _) => Some(0.0),
            (Verdict::Routed, Some(penalty)) => Some(penalty * (1.0 + history)),
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

    /// A step from `from` to `to`, if it may be taken: in one of [`DIRS`], which
    /// [`StepCache`] keeps the verdicts of, or a step to a pad's centre or onto its lattice,
    /// with `snap` if it is to the centre.
    fn track_step(
        &self,
        from: DNode,
        to: Point,
        out: &mut Vec<Step<DNode>>,
        snap: bool,
        dir: Option<(usize, u32)>,
    ) {
        let b = self.window.as_ref().unwrap_or(&self.params.bounds);
        if to.x < b.min().x || to.y < b.min().y || to.x > b.max().x || to.y > b.max().y {
            return;
        }
        let factor = if snap {
            Some(1.0)
        } else {
            self.layer_factor(to, from.l)
        };
        let Some(factor) = factor else { return };
        let factor = factor * self.world.plane_factor(self.ctx.net, from.l, to);
        let verdict = match dir.and_then(|(d, bits)| StepCache::known(bits, d)) {
            Some(v) => v,
            None => {
                let width = self.ctx.widths[usize::from(from.l)];
                let Ok(shape) = Shape::track(from.point(), to, width) else {
                    return;
                };
                let v = self
                    .world
                    .track(self.ctx.net, LayerId(from.l), &shape, None);
                if let Some((d, _)) = dir {
                    // The same track, the other way.
                    self.cache.set(from, d, v);
                    let back = DNode {
                        x: to.x,
                        y: to.y,
                        l: from.l,
                    };
                    self.cache.set(back, (d + 4) % 8, v);
                }
                v
            }
        };
        let history = self.history(to);
        let Some(extra) = self.verdict_cost(verdict, history) else {
            return;
        };
        let len = octile(from.point(), to) * NM;
        if let Some(cost) = Cost::new(len * (factor + history) + extra) {
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
    type Slots = KeptSlots;
    type ResourceSlots = DenseSlots<NodeId>;

    fn new_slots(&self) -> Self::Slots {
        KeptSlots::default()
    }

    fn new_resource_slots(&self) -> Self::ResourceSlots {
        DenseSlots::new()
    }

    fn contains(&self, node: DNode) -> bool {
        usize::from(node.l) < self.layers || node == TREE
    }

    fn expand(&self, _net: NetId, from: DNode, out: &mut Vec<Step<DNode>>) {
        if from.l == TREE_LAYER {
            return;
        }
        if let Some(run) = self.run {
            run.tick();
        }
        if self.to_tree && self.tree.contains_key(&from) {
            out.push(Step {
                to: TREE,
                cost: Cost::ZERO,
            });
            return;
        }
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
        let near = |v: i64, c: i64| c + (v - c + pitch / 2).div_euclid(pitch) * pitch;
        let on = |c: Point| (p.x - c.x) % pitch == 0 && (p.y - c.y) % pitch == 0;
        let mut found = self.near.borrow_mut();
        found.clear();
        let cell = self.ctx.pad_cell;
        if let Some(pads) = self
            .ctx
            .pad_index
            .get(&(p.x.div_euclid(cell), p.y.div_euclid(cell)))
        {
            let here = Rect::new(p, p);
            let close = |&&i: &&u32| {
                let b = self.ctx.pads[i as usize].bbox.expand(self.ctx.snap);
                b.intersects(&here)
            };
            found.extend(pads.iter().filter(close));
        }
        // The lattice goes on from here if it is the net's own, or that of a pad nearby.
        // Anywhere else, a pad's lattice has only the way back onto the net's: a net of
        // many pads would otherwise be searched for over the board once for each of them.
        let main = on(self.ctx.origin);
        if main || found.iter().any(|&i| on(self.ctx.pads[i as usize].center)) {
            let bits = self.cache.bits(from);
            for (d, (dx, dy)) in DIRS.into_iter().enumerate() {
                let to = Point::new(p.x + dx * pitch, p.y + dy * pitch);
                self.track_step(from, to, out, false, Some((d, bits)));
            }
        }
        if !main {
            let o = self.ctx.origin;
            self.track_step(
                from,
                Point::new(near(p.x, o.x), near(p.y, o.y)),
                out,
                false,
                None,
            );
        }
        // Straight to the centre of a pad nearby, or onto the lattice of its centre, so as
        // to reach it straight.
        {
            for &i in found.iter() {
                let pad = &self.ctx.pads[i as usize];
                // On a layer the pad isn't on, a route can still go to its centre for a via
                // in the pad.
                let under =
                    self.params.under_pads && self.ctx.via.is_some() && !self.reached[pad.terminal];
                if pad.center == p || !(pad.layers.contains(LayerId(from.l)) || under) {
                    continue;
                }
                let b = pad.bbox.expand(pitch);
                if b.intersects(&Rect::new(p, p)) {
                    self.track_step(from, pad.center, out, true, None);
                } else {
                    let m = Point::new(near(p.x, pad.center.x), near(p.y, pad.center.y));
                    if m != p && !out.iter().any(|s| s.to.l == from.l && s.to.point() == m) {
                        self.track_step(from, m, out, false, None);
                    }
                }
            }
        }
        drop(found);
        // Straight onto the tree, from less than a pitch away on another lattice.
        if self.to_tree {
            let (cx, cy) = (p.x.div_euclid(pitch), p.y.div_euclid(pitch));
            for (i, j) in (-1..=1).flat_map(|i| (-1..=1).map(move |j| (i, j))) {
                let Some(cell) = self.tree_cells.get(&(cx + i, cy + j, from.l)) else {
                    continue;
                };
                for &q in cell {
                    if (q.x - p.x).abs() < pitch
                        && (q.y - p.y).abs() < pitch
                        && !out.iter().any(|s| s.to.l == from.l && s.to.point() == q)
                    {
                        self.track_step(from, q, out, false, None);
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
            let verdict = if self.keep_off.iter().any(|&q| self.vias_clash(p, q)) {
                Verdict::Blocked
            } else {
                *self
                    .cache
                    .vias
                    .borrow_mut()
                    .entry(p)
                    .or_insert_with(|| self.world.via(self.ctx.net, p, diameter, drill, None))
            };
            if let Some(extra) = self.verdict_cost(verdict, self.history(p))
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
        let length = if from.l == TREE_LAYER {
            0.0
        } else if to.l == TREE_LAYER {
            self.to_tree_bound(from.point())
        } else {
            octile(from.point(), to.point())
        };
        // A little under, so that the sum of many steps rounded can't fall below it.
        Cost::new(length * NM * (1.0 - 1e-9)).unwrap_or(Cost::ZERO)
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
    /// The terminals a [partial](Params::partial) route gave up on.
    pub(crate) missing: Vec<usize>,
    /// Whether a search for one of those ran out of nodes, rather than of places to go.
    pub(crate) out_of_budget: bool,
}

/// Why a net could not be routed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// No path, within the corridor if there was one.
    NoPath,
    /// The expansion budget ran out.
    Budget,
}

/// How many connections a net's search budget, [`Params::max_expansions`], is for: a net
/// with more has that budget for each of them or fewer, so that a ground net of a hundred
/// pads doesn't run out where a net of two wouldn't.
pub(crate) const BUDGET_TERMINALS: usize = 64;

/// How many terminals a net has at most to fail whole: a net of more, such as a supply or
/// ground routed as tracks, that rip-up can't route whole is searched for a terminal at a
/// time, and keeps what it reaches.
pub(crate) const STUB_TERMINALS: usize = 16;

/// What share of [`Params::max_expansions`] a partial route's search from one terminal
/// may expand.
const PARTIAL_SHARE: u64 = 8;

/// The terminals other than the first, nearest the first first: by the distance between
/// the centres of their nearest pads.
fn stub_order(ctx: &NetCtx) -> Vec<usize> {
    let first: Vec<Point> = ctx
        .pads
        .iter()
        .filter(|p| p.terminal == 0)
        .map(|p| p.center)
        .collect();
    let mut near = vec![f64::INFINITY; ctx.terminals];
    for p in ctx.pads.iter().filter(|p| p.terminal != 0) {
        for &q in &first {
            near[p.terminal] = near[p.terminal].min(octile(p.center, q));
        }
    }
    let mut order: Vec<usize> = (1..ctx.terminals).collect();
    order.sort_by(|&a, &b| near[a].total_cmp(&near[b]).then(a.cmp(&b)));
    order
}

/// How many nodes a search for a connection may expand, and then a look from each of the
/// terminals it is for, before the next of these and at last the search with its whole
/// budget: first a glance from the terminals alone. Most connections are found by the first
/// search. One that isn't may be to a terminal that other nets' routes shut in, which a
/// search from the terminal finds out in a few nodes, where the search to it has to go
/// everywhere else first.
const LOOKS: [(u64, u64); 3] = [(0, 256), (8_000, 2_000), (64_000, 50_000)];

/// How many pitches around a terminal and the tree a search that may cross other nets'
/// routes is kept to at first. What crossing a route costs is many times the length of the
/// path, so such a search goes everywhere it can at each number of routes crossed before it
/// crosses one more: over the whole board, for a path that crosses what is in its way a few
/// millimetres from where it started.
const WINDOW: i64 = 24;

/// How many [`LOOKS`] a terminal is taken to have passed once a look from it reached the
/// tree.
const REACHED: u8 = u8::MAX;

/// Routes one net: a tree from its first terminal that reaches all of the others, each
/// search going from the whole tree so far to the nearest terminal not yet reached.
///
/// `cache` may be shared by searches of the net with the same world and pitch.
pub(crate) fn route_net<'a>(
    world: &'a World<'a>,
    ctx: &'a NetCtx,
    cache: &'a StepCache,
    params: Params,
    corridor: Option<&'a Corridor>,
    rip: Option<RipUp<'a>>,
) -> Result<Tree, (Failure, u64)> {
    let mut run = NetRun::new(world, ctx, cache, params);
    match run.stage(params, corridor, rip) {
        Ok(()) => Ok(run.finish()),
        Err(f) => Err((f, run.finish().expanded)),
    }
}

/// A net being routed: its tree so far, which [stages](NetRun::stage) grow, each from where
/// the one before stopped, so that what a search in the corridor connected is not searched
/// for again over the board, nor what that connected when routes may be crossed.
pub(crate) struct NetRun<'a> {
    space: DetailSpace<'a>,
    search: Search<DetailSpace<'a>>,
    back: Search<DetailSpace<'a>>,
    ctx: &'a NetCtx,
    tree: Tree,
    connected: Vec<bool>,
    /// Terminals given up on, in a partial route.
    missing: Vec<bool>,
    /// By terminal, how many of the [`LOOKS`] from it went their whole way without finding
    /// it shut in. The tree only grows, so what was open stays so.
    open: Vec<u8>,
    /// By terminal, whether a look from it that may cross other nets' routes went its whole
    /// way, or found it shut in by what may not be crossed. Neither changes for as long as
    /// the tree does not reach it.
    alive: Vec<bool>,
    dead: Vec<bool>,
    /// How many nodes the last search from the tree expanded to find that it goes nowhere
    /// further, if it did and the tree hasn't grown since.
    room: Option<u64>,
    /// The tree's nodes, in the order they joined it.
    sources: Vec<DNode>,
    retries: u32,
}

impl<'a> NetRun<'a> {
    /// A net with nothing but its first terminal. The pitch of `params` is that of every
    /// stage.
    pub(crate) fn new(
        world: &'a World<'a>,
        ctx: &'a NetCtx,
        cache: &'a StepCache,
        params: Params,
    ) -> NetRun<'a> {
        let space = DetailSpace::new(world, ctx, cache, params, None, None);
        let search = Search::new(&space);
        let back = Search::new(&space);
        let mut run = NetRun {
            space,
            search,
            back,
            ctx,
            tree: Tree::default(),
            connected: vec![false; ctx.terminals],
            missing: vec![false; ctx.terminals],
            open: vec![0; ctx.terminals],
            alive: vec![false; ctx.terminals],
            dead: vec![false; ctx.terminals],
            room: None,
            sources: Vec::new(),
            retries: 0,
        };
        run.join(0);
        run
    }

    /// Marks a terminal as reached, with every node of it on the tree: a terminal reached
    /// on one layer of a pad is on the tree on all of them.
    fn join(&mut self, terminal: usize) {
        if let Some(c) = self.connected.get_mut(terminal) {
            *c = true;
            self.space.reached[terminal] = true;
        }
        let start = self.space.tree_boxes.len();
        for n in self.ctx.terminal_nodes(terminal) {
            if self.space.grow(n) {
                self.sources.push(n);
                self.space.grow_box(Rect::new(n.point(), n.point()));
            }
        }
        self.space.group_boxes(start);
    }

    /// Charges the run, if there is one, for the nodes a search expanded.
    fn charge(&self, expanded: u64) {
        if let Some(run) = self.space.run {
            run.charge(expanded);
        }
    }

    /// The terminals still to reach.
    fn left(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.ctx.terminals).filter(|&t| !self.connected[t] && !self.missing[t])
    }

    /// The tree, with the terminals it doesn't reach as [`Tree::missing`].
    pub(crate) fn finish(mut self) -> Tree {
        self.tree.missing = (0..self.ctx.terminals)
            .filter(|&t| !self.connected[t])
            .collect();
        self.tree
    }

    /// A search from the tree to the terminals `to`, within `limit` nodes. Returns the path
    /// and the nodes expanded.
    fn forward(&mut self, to: &[usize], limit: u64) -> (Result<Vec<DNode>, Failure>, u64) {
        let ctx = self.ctx;
        let targets = to.iter().flat_map(|&t| ctx.terminal_nodes(t));
        // The tree's nodes are the sources of every search from it, and each would ask for
        // its bound to every terminal left again.
        self.search.reuse_bounds();
        let outcome = self.search.find_path_with(
            &self.space,
            ctx.net,
            self.sources.iter().map(|&n| Source::new(n)),
            targets.map(Target::new),
            control::limits(self.space.run, limit),
        );
        let path = match outcome {
            SearchOutcome::Found(found) => Ok(found.path.to_vec()),
            SearchOutcome::LimitReached | SearchOutcome::Cancelled => Err(Failure::Budget),
            SearchOutcome::Unreachable => Err(Failure::NoPath),
        };
        let expanded = self.search.stats().expanded;
        self.tree.expanded += expanded;
        self.charge(expanded);
        if path == Err(Failure::NoPath) && !self.space.crossing() && self.space.corridor.is_none() {
            self.room = Some(expanded);
        }
        (path, expanded)
    }

    /// A search from a terminal to the tree, within `limit` nodes, that may cross other
    /// nets' routes if `cross` and the stage's searches may. Returns the path, from the
    /// tree to the terminal, and the nodes expanded.
    fn backward(
        &mut self,
        terminal: usize,
        limit: u64,
        cross: bool,
    ) -> (Result<Vec<DNode>, Failure>, u64) {
        let ctx = self.ctx;
        self.space.to_tree = true;
        self.space.cross = cross;
        let outcome = self.back.find_path_with(
            &self.space,
            ctx.net,
            ctx.terminal_nodes(terminal).into_iter().map(Source::new),
            [Target::new(TREE)],
            control::limits(self.space.run, limit),
        );
        let path = match outcome {
            SearchOutcome::Found(found) => {
                let mut path: Vec<DNode> = found.path.to_vec();
                path.retain(|n| n.l != TREE_LAYER);
                path.reverse();
                Ok(path)
            }
            SearchOutcome::LimitReached | SearchOutcome::Cancelled => Err(Failure::Budget),
            SearchOutcome::Unreachable => Err(Failure::NoPath),
        };
        self.space.to_tree = false;
        self.space.cross = true;
        let expanded = self.back.stats().expanded;
        self.tree.expanded += expanded;
        self.charge(expanded);
        (path, expanded)
    }

    /// The box around the pads of `terminals` and the part of the tree nearest them, or
    /// the whole tree if `whole`, grown by [`WINDOW`] pitches, and no further than the
    /// bounds.
    fn window(&self, terminals: &[usize], whole: bool) -> Rect {
        let ctx = self.ctx;
        let pads = ctx
            .pads
            .iter()
            .filter(|p| terminals.contains(&p.terminal))
            .map(|p| p.bbox)
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default();
        let gap = |g: &Rect| {
            let dx = (g.min().x - pads.max().x)
                .max(pads.min().x - g.max().x)
                .max(0);
            let dy = (g.min().y - pads.max().y)
                .max(pads.min().y - g.max().y)
                .max(0);
            (dx.max(dy), dx.min(dy))
        };
        let groups = &self.space.tree_groups;
        let tree = if whole {
            groups.iter().copied().reduce(|a, b| a.union(&b))
        } else {
            groups.iter().copied().min_by_key(gap)
        };
        let all = tree.map_or(pads, |t| t.union(&pads));
        let grown = all.expand(WINDOW * self.space.params.pitch);
        let b = self.space.params.bounds;
        Rect::new(
            Point::new(grown.min().x.max(b.min().x), grown.min().y.max(b.min().y)),
            Point::new(grown.max().x.min(b.max().x), grown.max().y.min(b.max().y)),
        )
    }

    /// A search that may cross other nets' routes, from the terminal if there is one and
    /// from the tree to the terminals `to` otherwise: first near them and the tree, and
    /// then, if that finds nothing, anywhere. `budget` is charged for both.
    fn cross(
        &mut self,
        from: Option<usize>,
        to: &[usize],
        full: u64,
        budget: &mut u64,
    ) -> Result<Vec<DNode>, Failure> {
        let window = match from {
            Some(t) => self.window(&[t], false),
            None => self.window(to, true),
        };
        let mut last = Err(Failure::NoPath);
        for within in [Some(window), None] {
            if within.is_none() && window == self.space.params.bounds {
                break;
            }
            self.space.window = within;
            let (path, expanded) = match from {
                Some(t) => self.backward(t, full, true),
                None => self.forward(to, full),
            };
            self.space.window = None;
            *budget = budget.saturating_sub(expanded);
            last = path;
            if last.is_ok() {
                break;
            }
        }
        last
    }

    /// One more connection: a path from the tree to one of the terminals `to`, with at most
    /// `full` nodes expanded by a search over everything, which `budget` is charged for.
    /// `searched` gets the terminals that a whole search from them found no path for, and
    /// why.
    fn connect(
        &mut self,
        to: &[usize],
        full: u64,
        budget: &mut u64,
        searched: &mut Vec<(usize, Failure)>,
    ) -> Result<Vec<DNode>, Failure> {
        let crossing = self.space.crossing();
        for (level, (ahead, back)) in LOOKS.into_iter().enumerate() {
            // No look goes further than the search may.
            let back = back.min(full);
            if !crossing && ahead > 0 {
                let ahead = ahead.min(full);
                let (path, expanded) = self.forward(to, ahead);
                if path != Err(Failure::Budget) || ahead == full {
                    *budget = budget.saturating_sub(expanded);
                    return path;
                }
            } else if crossing && self.room.is_some_and(|n| n <= back) {
                // The tree is the one shut in, in less room than a look from a terminal
                // would go through: the search from it is the one to cross.
                break;
            }
            // A look from the terminals, without crossing anything. One shut in ends the
            // search for it, or, when routes may be crossed, is where to search from: the
            // first route crossed is then one that shuts it in, with no need to go
            // everywhere else first. One that reaches the tree only says that the search
            // from the tree will reach it: that search is for the nearest terminal of all,
            // and this one was for the first.
            let mut any_open = false;
            for &t in to {
                if usize::from(self.open[t]) == level && !searched.iter().any(|s| s.0 == t) {
                    match self.backward(t, back, false).0 {
                        Ok(path) if crossing => return Ok(path),
                        Ok(_) => self.open[t] = REACHED,
                        Err(Failure::Budget) => self.open[t] += 1,
                        Err(Failure::NoPath) => {
                            if crossing {
                                match self.cross(Some(t), to, full, budget) {
                                    Ok(path) => return Ok(path),
                                    Err(why) => {
                                        searched.push((t, why));
                                        self.dead[t] = why == Failure::NoPath;
                                    }
                                }
                            }
                        }
                    }
                }
                if usize::from(self.open[t]) > level {
                    any_open = true;
                    if !crossing {
                        break;
                    }
                }
            }
            if !crossing {
                if !any_open {
                    return Err(Failure::NoPath);
                }
                if to.iter().any(|&t| self.open[t] == REACHED) {
                    break;
                }
            }
        }
        // What is shut in by what may not be crossed can't be reached from the tree either,
        // which a search from the tree would go over the whole board to find out.
        let mut to: Vec<usize> = to.to_vec();
        if crossing {
            for &t in &to {
                if !self.alive[t] && !self.dead[t] {
                    match self.backward(t, LOOKS[1].1.min(full), true).0 {
                        Ok(path) => return Ok(path),
                        Err(Failure::Budget) => self.alive[t] = true,
                        Err(Failure::NoPath) => self.dead[t] = true,
                    }
                }
            }
            to.retain(|&t| !self.dead[t]);
            if to.is_empty() {
                return Err(Failure::NoPath);
            }
            return self.cross(None, &to, full, budget);
        }
        let (path, expanded) = self.forward(&to, full);
        *budget = budget.saturating_sub(expanded);
        path
    }

    /// Grows the tree to the terminals it has still to reach, as `params` say, in
    /// `corridor` if there is one, and as `rip` negotiates with other nets. What it
    /// connects stays connected if it fails, for a later stage to go on from.
    pub(crate) fn stage(
        &mut self,
        params: Params,
        corridor: Option<&'a Corridor>,
        rip: Option<RipUp<'a>>,
    ) -> Result<(), Failure> {
        let ctx = self.ctx;
        debug_assert_eq!(params.pitch, self.space.params.pitch);
        self.space.params = params;
        self.space.corridor = corridor;
        self.space.rip = rip;
        // The budget is for each [`BUDGET_TERMINALS`] connections, or fewer.
        let mut budget = params
            .max_expansions
            .saturating_mul((ctx.terminals.saturating_sub(2) / BUDGET_TERMINALS + 1) as u64);
        // Stubs are searched for one terminal at a time, nearest the first terminal first.
        let order = if params.stubs {
            stub_order(ctx)
        } else {
            Vec::new()
        };
        while self.left().next().is_some() {
            let stub = order
                .iter()
                .copied()
                .find(|&t| !self.connected[t] && !self.missing[t]);
            let to: Vec<usize> = match stub {
                Some(t) => vec![t],
                None => self.left().collect(),
            };
            let full = if stub.is_some() {
                budget.min(params.max_expansions / PARTIAL_SHARE)
            } else {
                budget
            };
            let mut searched = Vec::new();
            let mut path = self.connect(&to, full, &mut budget, &mut searched);
            // A partial route searches from each terminal left to the tree on its own, with a
            // budget of its own, and gives up on those it doesn't reach.
            if path.is_err() && params.partial {
                for t in to {
                    let why = match searched.iter().find(|s| s.0 == t) {
                        Some(&(_, why)) => why,
                        None => {
                            let limit = params.max_expansions / PARTIAL_SHARE;
                            match self.backward(t, limit, true).0 {
                                Ok(found) => {
                                    path = Ok(found);
                                    break;
                                }
                                Err(why) => why,
                            }
                        }
                    };
                    if why == Failure::Budget {
                        self.tree.out_of_budget = true;
                    }
                    self.missing[t] = true;
                }
                if path.is_err() {
                    if stub.is_some() {
                        continue;
                    }
                    break;
                }
            }
            let path = path?;
            // The path's vias, which must keep clear of each other and of the net's others.
            let mut new_vias: Vec<Point> = Vec::new();
            for w in path.windows(2) {
                let p = w[0].point();
                if w[0].l != w[1].l && !ctx.joins(p, w[0].l, w[1].l) && !new_vias.contains(&p) {
                    new_vias.push(p);
                }
            }
            let clash = new_vias.iter().enumerate().find_map(|(i, &p)| {
                let earlier = new_vias[..i].iter().chain(&self.space.keep_off);
                earlier
                    .clone()
                    .any(|&q| self.space.vias_clash(p, q))
                    .then_some(p)
            });
            if let Some(p) = clash {
                self.retries += 1;
                if self.retries > 16 {
                    if params.partial {
                        break;
                    }
                    return Err(Failure::NoPath);
                }
                self.space.keep_off(p);
                continue;
            }
            for &p in &new_vias {
                self.space.keep_off(p);
                // A via joins every layer.
                for l in 0..self.space.layers as u8 {
                    let n = DNode { x: p.x, y: p.y, l };
                    if self.space.grow(n) {
                        self.sources.push(n);
                    }
                }
            }
            for w in path.windows(2) {
                self.tree.edges.push((w[0], w[1]));
            }
            self.space.grow_boxes(&path);
            self.room = None;
            let before = self.connected.iter().filter(|&&c| c).count();
            for &n in &path {
                if self.space.grow(n) {
                    self.sources.push(n);
                }
                let terminal = ctx
                    .centers
                    .get(&(n.point(), n.l))
                    .map(|&i| ctx.pads[i].terminal);
                if let Some(t) = terminal
                    && !self.connected[t]
                {
                    self.join(t);
                }
            }
            if self.connected.iter().filter(|&&c| c).count() == before {
                // Can't happen, as every path ends at a terminal's node, but a loop must
                // not hang.
                if params.partial {
                    break;
                }
                return Err(Failure::NoPath);
            }
        }
        Ok(())
    }
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
