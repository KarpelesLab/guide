//! Differential pairs: the two nets of a pair routed side by side, at a constant gap.
//!
//! Pairs are routed last, in place of the routes their two nets were given as any others,
//! so that they can't take room another net needed: each pair is taken out, routed side by
//! side where it fits, and kept if it connects everything its nets did, or put back.
//!
//! A pair is routed between two *ends*, each a pad of either net close together, such as
//! the two pins of a USB connector. A search finds the pair's centreline from one end to
//! the other, over a lattice whose steps turn by 45° at most, checking at each step that a
//! track as wide as both tracks and the gap between them fits, with a margin for the
//! corners. The centreline is then offset to either side into the two tracks, mitred at
//! the corners, and a layer change into a via of each net, side by side, after a straight
//! run on either side, with a 45° jog out to the vias where they are farther apart than the
//! tracks. Near the ends, where the pins of a footprint leave no room for both tracks, the
//! search goes unchecked, and that part of the centreline is dropped: each net's track
//! there, its *fan-out*, is routed on its own, from its pad to its end of the coupled run.
//! Vias cost ten times as much as in routing, as in cleanup, since the pair replaces routes
//! that cleanup has had; so where the two ends are on different layers, as a BGA's balls on
//! top and an edge connector's fingers below, the search's lower bound counts the layer
//! change it can't do without, or it would search most of the board first.
//!
//! The coupled run and the fan-outs are checked exactly, net by net, against everything
//! else and each other. If anything fails, the pair keeps the routes it had.
//!
//! A pair routed first that a net left for the finer lattice takes the place of is routed
//! again in rip-up: its search may cross other nets' routes at a price, as a net's does,
//! and those its copper runs into are taken out, to be routed again after it.

use core::cell::RefCell;

use super::Design;
use super::cleanup::{VIA_COST_FACTOR, pull_tight};
use super::detail::{self, DIRS, NM, NetCtx, Params, RipUp, StepCache, TermPad, octile};
use super::geometry::NetCopper;
use super::router::{Job, LATTICE_REACH};
use super::world::{Placed, Verdict, World};
use crate::cost::Cost;
use crate::geom::drc::{Track, Via};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::{NetId, NodeId};
use crate::search::{Search, SearchOutcome, Source, Target};
use crate::space::{DenseSlots, FastMap, InternedSlots, RoutingSpace, Step};

/// How far apart the two nets' pads at an end may be, for the pair to be routed side by
/// side from there: farther, and the two nets are routed apart.
pub(crate) const MAX_SPLIT: i64 = 5_000_000;

/// The copper of a pair routed side by side.
pub(crate) struct PairRoute {
    /// The tracks of both nets.
    pub(crate) tracks: Vec<Track>,
    /// The vias of both nets.
    pub(crate) vias: Vec<Via>,
    /// For the positive and then the negative net, the terminals it joins, by their
    /// numbers in the net's job.
    pub(crate) terminals: [[usize; 2]; 2],
    /// What it put into the world.
    pub(crate) undo: Undo,
}

/// A point on a layer, reached in a direction: one of [`DIRS`], or 8 for none yet. Vias
/// need a straight run on either side, so a node also has how many steps it has gone
/// straight on since it last turned or changed layers, as far as that matters, and whether
/// it last changed layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PNode {
    x: i64,
    y: i64,
    l: u8,
    d: u8,
    run: u8,
    via: bool,
}

impl PNode {
    fn point(self) -> Point {
        Point::new(self.x, self.y)
    }
}

/// The unit vector of a direction.
fn unit(d: usize) -> (f64, f64) {
    let (x, y) = DIRS[d];
    let len = if x != 0 && y != 0 {
        core::f64::consts::SQRT_2
    } else {
        1.0
    };
    (x as f64 / len, y as f64 / len)
}

/// The unit vector from `a` to `b`.
fn unit_between(a: Point, b: Point) -> (f64, f64) {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let len = (dx * dx + dy * dy).sqrt();
    (dx / len, dy / len)
}

/// `p` moved by a vector, rounded to the nanometre.
fn shift(p: Point, v: (f64, f64)) -> Point {
    Point::new(p.x + v.0.round() as i64, p.y + v.1.round() as i64)
}

/// A polyline offset by `h` to its left, with mitred corners: the line `h` from it on that
/// side, as a track of a pair is from the pair's centreline.
pub(crate) fn offset(points: &[Point], h: f64) -> Vec<Point> {
    let n = points.len();
    let normal = |i: usize| {
        let (ux, uy) = unit_between(points[i], points[i + 1]);
        (-uy, ux)
    };
    (0..n)
        .map(|i| {
            let v = if i == 0 {
                let m = normal(0);
                (m.0 * h, m.1 * h)
            } else if i == n - 1 {
                let m = normal(n - 2);
                (m.0 * h, m.1 * h)
            } else {
                let (a, b) = (normal(i - 1), normal(i));
                let k = h / (1.0 + a.0 * b.0 + a.1 * b.1);
                ((a.0 + b.0) * k, (a.1 + b.1) * k)
            };
            shift(points[i], v)
        })
        .collect()
}

/// The worst of some verdicts: blocked if any is, or else in the way of routes if any is.
fn worst(verdicts: &[Verdict]) -> Verdict {
    if verdicts.contains(&Verdict::Blocked) {
        Verdict::Blocked
    } else if verdicts.contains(&Verdict::Routed) {
        Verdict::Routed
    } else {
        Verdict::Free
    }
}

/// Drops the points of a polyline where it goes straight on.
fn straighten(points: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for &p in points {
        if out.last() == Some(&p) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let (ux, uy) = ((b.x - a.x) as i128, (b.y - a.y) as i128);
            let (vx, vy) = ((p.x - b.x) as i128, (p.y - b.y) as i128);
            if ux * vy == uy * vx && ux * vx + uy * vy > 0 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// The sizes a pair is routed at.
struct Geometry {
    nets: (NetId, NetId),
    /// The width of the two tracks, by layer.
    widths: Vec<i64>,
    /// The gap between them.
    gap: i64,
    /// From the centreline to each track's centre.
    half: i64,
    /// The width of the track that stands for both in the search.
    footprint: i64,
    /// The vias of the two nets, as diameter and drill, if both have them.
    vias: Option<((i64, i64), (i64, i64))>,
    /// From the centreline to each via's centre.
    via_half: i64,
}

/// Where the search may go unchecked: near an end, where the two nets fan out.
#[derive(Clone, Copy, Debug)]
struct Disk {
    center: Point,
    radius: i64,
}

impl Disk {
    fn contains(&self, p: Point) -> bool {
        let (dx, dy) = ((p.x - self.center.x) as i128, (p.y - self.center.y) as i128);
        dx * dx + dy * dy <= i128::from(self.radius) * i128::from(self.radius)
    }
}

/// The space of a pair's centreline.
struct PairSpace<'a> {
    world: &'a World<'a>,
    geo: &'a Geometry,
    pitch: i64,
    bounds: Rect,
    via_cost: f64,
    layers: usize,
    /// How many steps a run goes straight on before and after a layer change.
    straight: u8,
    ends: [Disk; 2],
    /// The second end's centre and the layers it may be reached on.
    target: (Point, LayerSet),
    /// Verdicts on steps, by node and direction, and on vias, by point and direction.
    steps: RefCell<FastMap<(i64, i64, u8, u8), Verdict>>,
    vias: RefCell<FastMap<(i64, i64, u8), Verdict>>,
    /// With a penalty, other nets' routes may be crossed at that price, to be ripped up.
    rip: Option<RipUp<'a>>,
    /// The run the search is part of, which hears of every node.
    run: Option<&'a super::control::Run<'a>>,
}

impl PairSpace<'_> {
    fn unchecked(&self, p: Point) -> bool {
        self.ends.iter().any(|d| d.contains(p))
    }

    /// What a step or a via with this verdict costs on top of its length, ending at `at`,
    /// or `None` if it may not be taken.
    fn extra(&self, verdict: Verdict, at: Point) -> Option<f64> {
        match (verdict, self.rip.and_then(|r| r.penalty)) {
            (Verdict::Free, _) => Some(0.0),
            (Verdict::Routed, Some(penalty)) => {
                let history = self.rip.map_or(0.0, |r| {
                    r.history
                        .grid
                        .tile_of(at)
                        .map_or(0.0, |t| r.history.cost[t])
                });
                Some(penalty * (1.0 + history))
            }
            _ => None,
        }
    }

    /// The two vias of a layer change at `p`, reached in direction `d`: to the left of the
    /// centreline first.
    fn via_points(&self, p: Point, d: usize) -> (Point, Point) {
        let (ux, uy) = unit(d);
        let h = self.geo.via_half as f64;
        (shift(p, (-uy * h, ux * h)), shift(p, (uy * h, -ux * h)))
    }

    fn step_verdict(&self, from: PNode, to: Point, d: usize) -> Verdict {
        let key = (from.x, from.y, from.l, d as u8);
        if let Some(&v) = self.steps.borrow().get(&key) {
            return v;
        }
        let v = Shape::track(from.point(), to, self.geo.footprint).map_or(Verdict::Blocked, |s| {
            self.world
                .track_pair(self.geo.nets, LayerId(from.l), &s, None)
        });
        let mut steps = self.steps.borrow_mut();
        steps.insert(key, v);
        steps.insert((to.x, to.y, from.l, ((d + 4) % 8) as u8), v);
        v
    }

    fn via_verdict(&self, p: Point, d: usize) -> Verdict {
        let Some(((da, ra), (db, rb))) = self.geo.vias else {
            return Verdict::Blocked;
        };
        let key = (p.x, p.y, d as u8);
        if let Some(&v) = self.vias.borrow().get(&key) {
            return v;
        }
        let (a, b) = self.via_points(p, d);
        let (pos, neg) = self.geo.nets;
        // Either net's via may end up on either side.
        let mut verdicts = Vec::new();
        for q in [a, b] {
            verdicts.push(self.world.via(pos, q, da, ra, None));
            verdicts.push(self.world.via(neg, q, db, rb, None));
        }
        // And the tracks jog out to them on every layer.
        let (ux, uy) = unit(d);
        let jog = (self.geo.via_half - self.geo.half) as f64;
        let wide = 2 * self.geo.via_half + self.geo.widths.iter().copied().max().unwrap_or(0);
        match Shape::track(
            shift(p, (-ux * jog, -uy * jog)),
            shift(p, (ux * jog, uy * jog)),
            wide,
        ) {
            Ok(s) => {
                for l in 0..self.layers {
                    let v = self
                        .world
                        .track_pair(self.geo.nets, LayerId(l as u8), &s, None);
                    verdicts.push(v);
                    if v == Verdict::Blocked {
                        break;
                    }
                }
            }
            Err(_) => verdicts.push(Verdict::Blocked),
        }
        let v = worst(&verdicts);
        self.vias.borrow_mut().insert(key, v);
        v
    }
}

impl RoutingSpace for PairSpace<'_> {
    type Node = PNode;
    type Resource = NodeId;
    type Slots = InternedSlots<PNode>;
    type ResourceSlots = DenseSlots<NodeId>;

    fn new_slots(&self) -> Self::Slots {
        InternedSlots::new()
    }

    fn new_resource_slots(&self) -> Self::ResourceSlots {
        DenseSlots::new()
    }

    fn contains(&self, node: PNode) -> bool {
        usize::from(node.l) < self.layers
    }

    fn expand(&self, _net: NetId, from: PNode, out: &mut Vec<Step<PNode>>) {
        if let Some(run) = self.run {
            run.tick();
        }
        let p = from.point();
        let here = self.unchecked(p);
        // Into the second end, from anywhere near it.
        if self.ends[1].contains(p)
            && self.target.1.contains(LayerId(from.l))
            && p != self.target.0
            && let Some(cost) = Cost::new(octile(p, self.target.0) * NM)
        {
            out.push(Step {
                to: PNode {
                    x: self.target.0.x,
                    y: self.target.0.y,
                    l: from.l,
                    d: 8,
                    run: self.straight,
                    via: false,
                },
                cost,
            });
        }
        let turns: &[usize] = if from.d == 8 {
            &[0, 1, 2, 3, 4, 5, 6, 7]
        } else if from.via && from.run < self.straight {
            &[0]
        } else {
            &[0, 1, 7]
        };
        let b = &self.bounds;
        for &t in turns {
            let d = (usize::from(from.d) + t) % 8;
            let (dx, dy) = DIRS[d];
            let to = Point::new(p.x + dx * self.pitch, p.y + dy * self.pitch);
            if to.x < b.min().x || to.y < b.min().y || to.x > b.max().x || to.y > b.max().y {
                continue;
            }
            let extra = if here || self.unchecked(to) {
                Some(0.0)
            } else {
                self.extra(self.step_verdict(from, to, d), to)
            };
            let Some(extra) = extra else { continue };
            if let Some(cost) = Cost::new(octile(p, to) * NM + extra) {
                out.push(Step {
                    to: PNode {
                        x: to.x,
                        y: to.y,
                        l: from.l,
                        d: d as u8,
                        run: if t == 0 {
                            (from.run + 1).min(self.straight)
                        } else {
                            0
                        },
                        via: t == 0 && from.via && from.run + 1 < self.straight,
                    },
                    cost,
                });
            }
        }
        if from.d < 8
            && from.run >= self.straight
            && !here
            && self.layers > 1
            && let Some(extra) = self.extra(self.via_verdict(p, usize::from(from.d)), p)
            && let Some(cost) = Cost::new(2.0 * self.via_cost + extra)
        {
            for l in (0..self.layers as u8).filter(|&l| l != from.l) {
                out.push(Step {
                    to: PNode {
                        l,
                        run: 0,
                        via: true,
                        ..from
                    },
                    cost,
                });
            }
        }
    }

    fn lower_bound(&self, _net: NetId, from: PNode, _to: PNode) -> Cost {
        // On a layer the second end isn't reached on, a layer change at least is to come.
        let change = if self.target.1.contains(LayerId(from.l)) {
            0.0
        } else {
            2.0 * self.via_cost
        };
        Cost::new((octile(from.point(), self.target.0) * NM + change) * (1.0 - 1e-9))
            .unwrap_or(Cost::ZERO)
    }

    fn resources(&self, _net: NetId, _from: PNode, _to: PNode, _out: &mut Vec<NodeId>) {}

    fn capacity(&self, _resource: NodeId) -> u32 {
        1
    }
}

/// The two ends of a pair: for each, a pad of either net, by position in its job's pads.
fn ends(p: &NetCtx, n: &NetCtx) -> Option<[(usize, usize); 2]> {
    if p.pads.len() > 64 || n.pads.len() > 64 {
        return None;
    }
    let far = |a: Point, b: Point| {
        let (dx, dy) = ((a.x - b.x) as i128, (a.y - b.y) as i128);
        dx * dx + dy * dy
    };
    let mid = |a: Point, b: Point| Point::new(a.x + (b.x - a.x) / 2, a.y + (b.y - a.y) / 2);
    let split = i128::from(MAX_SPLIT) * i128::from(MAX_SPLIT);
    let couples: Vec<(usize, usize)> = (0..p.pads.len())
        .flat_map(|i| (0..n.pads.len()).map(move |j| (i, j)))
        .filter(|&(i, j)| far(p.pads[i].center, n.pads[j].center) <= split)
        .collect();
    let mut best: Option<(i128, [(usize, usize); 2])> = None;
    for (k, &(i1, j1)) in couples.iter().enumerate() {
        for &(i2, j2) in &couples[k + 1..] {
            if p.pads[i1].terminal == p.pads[i2].terminal
                || n.pads[j1].terminal == n.pads[j2].terminal
            {
                continue;
            }
            let d = far(
                mid(p.pads[i1].center, n.pads[j1].center),
                mid(p.pads[i2].center, n.pads[j2].center),
            );
            if best.is_none_or(|(b, _)| d > b) {
                best = Some((d, [(i1, j1), (i2, j2)]));
            }
        }
    }
    best.map(|(_, e)| e)
}

/// The fewest pads an array has, such as a BGA's.
const MIN_ARRAY: usize = 8;

/// The box of the array of pads that the pad at `at` is in, if it is in one: pads of any
/// net closer to each other than `passage`, which a pair can't pass between, at least
/// [`MIN_ARRAY`] of them.
fn array_reach(design: &Design, at: Point, passage: i64) -> Option<Rect> {
    let pads = &design.board.pads;
    let start = pads
        .iter()
        .position(|p| p.shape.bbox().intersects(&Rect::new(at, at)))?;
    let mut seen = vec![false; pads.len()];
    seen[start] = true;
    let mut queue = vec![start];
    let mut bbox = pads[start].shape.bbox();
    let mut count = 0;
    while let Some(i) = queue.pop() {
        count += 1;
        let reach = pads[i].shape.bbox().expand(passage);
        for (j, q) in pads.iter().enumerate() {
            if !seen[j]
                && q.layers.0 & pads[i].layers.0 != 0
                && q.shape.bbox().intersects(&reach)
                && q.shape.violates(&pads[i].shape, passage) == Ok(true)
            {
                seen[j] = true;
                bbox = bbox.union(&q.shape.bbox());
                queue.push(j);
            }
        }
    }
    (count >= MIN_ARRAY).then_some(bbox)
}

/// Half the diagonal of a box.
fn half_diagonal(r: &Rect) -> i64 {
    let (w, h) = (
        (r.max().x - r.min().x) as f64,
        (r.max().y - r.min().y) as f64,
    );
    ((w * w + h * h).sqrt() / 2.0).ceil() as i64
}

/// What was put into the world for a pair, to take out again.
#[derive(Default)]
pub(crate) struct Undo {
    placed: Vec<Placed>,
    copper: Vec<NetCopper>,
}

impl Undo {
    pub(crate) fn undo(mut self, world: &mut World<'_>) {
        for p in &self.placed {
            world.remove(p);
        }
        for c in &mut self.copper {
            c.unplace(world);
        }
    }

    /// Puts a pair's tracks and vias into the world as routes of their nets.
    pub(crate) fn place(world: &mut World<'_>, tracks: &[Track], vias: &[Via]) -> Undo {
        let mut undo = Undo::default();
        for t in tracks {
            if let (Some(net), Ok(s)) = (t.net, Shape::track(t.start, t.end, t.width)) {
                undo.placed.push(world.add_track(net, t.layer, s));
            }
        }
        for v in vias {
            if let Some(net) = v.net {
                undo.placed
                    .push(world.add_via(net, v.at, v.diameter, v.drill));
            }
        }
        undo
    }
}

/// How a pair routed among others' routes may move them: crossing them at a price, as a net
/// in rip-up does, and taking them out of the world with `take`, which the caller routes
/// again.
pub(crate) struct PairRip<'h, 'f> {
    pub(crate) rip: RipUp<'h>,
    pub(crate) take: &'f mut dyn FnMut(&mut World<'_>, &[NetId]),
}

/// Routes one pair side by side into `world`, from the jobs of its two nets, if it can.
/// With `rip`, other nets' routes in its way are taken out.
pub(crate) fn route_pair(
    design: &Design,
    world: &mut World<'_>,
    pair: &super::DiffPair,
    pj: &Job,
    nj: &Job,
    params: Params,
    mut rip: Option<PairRip<'_, '_>>,
) -> Option<PairRoute> {
    // The pair's own copper is never in its way to be moved.
    let nets = [pj.ctx.net, nj.ctx.net];
    let was = nets.map(|n| world.locked(n));
    for n in nets {
        world.set_locked(n, true);
    }
    let route = route_pair_locked(design, world, pair, pj, nj, params, &mut rip);
    for (n, w) in nets.into_iter().zip(was) {
        world.set_locked(n, w);
    }
    route
}

fn route_pair_locked(
    design: &Design,
    world: &mut World<'_>,
    pair: &super::DiffPair,
    pj: &Job,
    nj: &Job,
    params: Params,
    rip: &mut Option<PairRip<'_, '_>>,
) -> Option<PairRoute> {
    let rules = &design.rules;
    let (pc, nc) = (&pj.ctx, &nj.ctx);
    let nets = (pc.net, nc.net);
    let layers = rules.layer_count();
    let gap = (0..layers)
        .map(|l| rules.clearance_between(Some(nets.0), Some(nets.1), LayerId(l as u8)))
        .fold(pair.gap, i64::max);
    let widths: Vec<i64> = (0..layers)
        .map(|l| {
            let l = LayerId(l as u8);
            pair.width
                .max(rules.track_width(Some(nets.0), l))
                .max(rules.track_width(Some(nets.1), l))
        })
        .collect();
    let width = widths.iter().copied().max()?;
    // A nanometre more than half the spacing, against rounding: each track's corners are
    // rounded to the nanometre, which moves its edge by 0.71 nm at most, so the two are
    // never closer than the gap, and no more than 3 nm further apart.
    let half = (width + gap + 1) / 2 + 1;
    // Mitred corners of 45° stand out by 8% of the half spacing on the outside.
    let footprint = 2 * half + width + (half + 9) / 10;
    let vias = pc.via.zip(nc.via).filter(|_| layers > 1);
    let via_half = vias.map_or(half, |((da, ra), (db, rb))| {
        let h2h = rules.spec().hole_to_hole;
        let copper = (da.max(db) + gap + 1) / 2 + 8;
        let holes = (ra.max(rb) + h2h + 1) / 2 + 8;
        half.max(copper).max(holes)
    });
    let geo = Geometry {
        nets,
        widths,
        gap,
        half,
        footprint,
        vias,
        via_half,
    };

    // The ends, and how far from each the search goes unchecked.
    let [(pa, na), (pb, nb)] = ends(pc, nc)?;
    let ends_at = [(pa, na), (pb, nb)];
    // As cleanup does, with dearer vias: the pair replaces routes that cleanup has had.
    let params = Params {
        via_cost: params.via_cost * VIA_COST_FACTOR,
        ..params
    };
    // Near the pads first, and if the pair can't get out from there, farther, and then out
    // of the array of pads it is in, such as a BGA's, which the two nets fan out of apart.
    let passage = footprint + 2 * rules.max_clearance();
    let arrays = [
        array_reach(design, pc.pads[pa].center, passage),
        array_reach(design, pc.pads[pb].center, passage),
    ];
    let extras = [footprint / 2 + params.pitch, footprint + params.pitch];
    for (round, extra) in extras
        .into_iter()
        .chain([footprint + params.pitch])
        .enumerate()
    {
        if round == 2 && arrays.iter().all(Option::is_none) {
            break;
        }
        let disk = |e: usize, (i, j): (usize, usize)| {
            let (a, b) = (&pc.pads[i], &nc.pads[j]);
            let center = Point::new(
                a.center.x + (b.center.x - a.center.x) / 2,
                a.center.y + (b.center.y - a.center.y) / 2,
            );
            let apart = octile(a.center, b.center) as i64 / 2;
            let mut radius = apart + half_diagonal(&a.bbox).max(half_diagonal(&b.bbox)) + extra;
            if round == 2
                && let Some(bbox) = arrays[e]
            {
                let out = (center.x - bbox.min().x)
                    .min(bbox.max().x - center.x)
                    .min(center.y - bbox.min().y)
                    .min(bbox.max().y - center.y)
                    .max(0);
                radius = radius.max(out + extra);
            }
            Disk { center, radius }
        };
        let disks = [disk(0, ends_at[0]), disk(1, ends_at[1])];
        if octile(disks[0].center, disks[1].center) as i64
            <= disks[0].radius + disks[1].radius + 2 * params.pitch
        {
            return None;
        }
        if let Some(route) = attempt(design, world, &geo, (pc, nc), ends_at, disks, params, rip) {
            return Some(route);
        }
    }
    None
}

/// One attempt at routing a pair, with the search unchecked in `disks` around its ends.
#[allow(clippy::too_many_arguments)]
fn attempt(
    design: &Design,
    world: &mut World<'_>,
    geo: &Geometry,
    (pc, nc): (&NetCtx, &NetCtx),
    [(pa, na), (pb, nb)]: [(usize, usize); 2],
    ends: [Disk; 2],
    params: Params,
    rip: &mut Option<PairRip<'_, '_>>,
) -> Option<PairRoute> {
    let layers = design.rules.layer_count();
    let nets = geo.nets;
    let (half, via_half) = (geo.half, geo.via_half);
    let on = |i: usize, j: usize| {
        let (a, b) = (pc.pads[i].layers, nc.pads[j].layers);
        if a.0 & b.0 != 0 {
            LayerSet(a.0 & b.0)
        } else {
            LayerSet(a.0 | b.0)
        }
    };
    let straight = ((via_half - half + geo.vias.map_or(0, |v| v.0.0.max(v.1.0)) + geo.gap)
        / params.pitch.max(1)
        + 1)
    .clamp(1, 8) as u8;
    let space = PairSpace {
        world,
        straight,
        geo,
        pitch: params.pitch,
        bounds: params.bounds,
        via_cost: params.via_cost,
        layers,
        ends,
        target: (ends[1].center, on(pb, nb)),
        steps: RefCell::new(FastMap::default()),
        vias: RefCell::new(FastMap::default()),
        rip: rip.as_ref().map(|r| r.rip),
        run: world.run(),
    };
    let run = world.run();
    let mut search = Search::new(&space);
    let sources: Vec<PNode> = on(pa, na)
        .iter()
        .map(|l| PNode {
            x: ends[0].center.x,
            y: ends[0].center.y,
            l: l.0,
            d: 8,
            run: straight,
            via: false,
        })
        .collect();
    let targets: Vec<PNode> = space
        .target
        .1
        .iter()
        .map(|l| PNode {
            x: ends[1].center.x,
            y: ends[1].center.y,
            l: l.0,
            d: 8,
            run: straight,
            via: false,
        })
        .collect();
    let outcome = search.find_path_with(
        &space,
        nets.0,
        sources.into_iter().map(Source::new),
        targets.into_iter().map(Target::new),
        super::control::limits(run, params.max_expansions),
    );
    let path: Option<Vec<PNode>> = match outcome {
        SearchOutcome::Found(found) => Some(found.path.to_vec()),
        _ => None,
    };
    if let Some(run) = run {
        run.charge(search.stats().expanded);
    }
    let path = path?;
    drop(search);
    drop(space);

    // The coupled run: from where the path last leaves the first end to where it next
    // enters the second, with no layer change at either end.
    let last_a = path.iter().rposition(|n| ends[0].contains(n.point()))?;
    let first_b = last_a
        + path[last_a..]
            .iter()
            .position(|n| ends[1].contains(n.point()))?;
    let (mut s, mut e) = (last_a + 1, first_b.checked_sub(1)?);
    while s < e && path[s].l != path[s + 1].l {
        s += 1;
    }
    while e > s && path[e].l != path[e - 1].l {
        e -= 1;
    }
    if e <= s {
        return None;
    }
    let run = &path[s..=e];

    // Which side the positive net is on: the side of its pad at the first end.
    let first_dir = run[1].d as usize;
    let (ux, uy) = unit(first_dir);
    let (pp, np) = (pc.pads[pa].center, nc.pads[na].center);
    let cross = ux * (pp.y - np.y) as f64 - uy * (pp.x - np.x) as f64;
    let side = if cross >= 0.0 { 1.0 } else { -1.0 };

    // Runs on each layer, between layer changes: the layer, the points, and the direction
    // of the layer change before and after, if any.
    type Run = (u8, Vec<Point>, Option<usize>, Option<usize>);
    let mut runs: Vec<Run> = vec![(run[0].l, vec![run[0].point()], None, None)];
    for w in run.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a.l == b.l {
            if let Some(r) = runs.last_mut() {
                r.1.push(b.point());
            }
        } else {
            if let Some(r) = runs.last_mut() {
                r.3 = Some(usize::from(a.d));
            }
            runs.push((b.l, vec![b.point()], Some(usize::from(a.d)), None));
        }
    }
    let mut copper: [(Vec<Track>, Vec<Via>); 2] = Default::default();
    let mut ends_at: [[(Point, u8); 2]; 2] = [[(Point::new(0, 0), 0); 2]; 2];
    for (k, &sign) in [side, -side].iter().enumerate() {
        let h = sign * geo.half as f64;
        let hv = sign * geo.via_half as f64;
        let net = if k == 0 { nets.0 } else { nets.1 };
        let via = geo.vias.map(|v| if k == 0 { v.0 } else { v.1 });
        let (tracks, vias) = &mut copper[k];
        for (i, (l, points, from_via, to_via)) in runs.iter().enumerate() {
            let points = straighten(points);
            if points.len() < 2 {
                return None;
            }
            let mut line = offset(&points, h);
            let at_via = |p: Point, d: usize| {
                let (ux, uy) = unit(d);
                shift(p, (-uy * hv, ux * hv))
            };
            // The tracks keep their gap up to a 45° jog out to their vias, where there is
            // room for it.
            let jog = (geo.via_half - geo.half) as f64;
            let len = |a: Point, b: Point| octile(a, b);
            if let Some(d) = from_via {
                if jog > 0.0 && len(line[0], line[1]) > 2.0 * jog {
                    let (ux, uy) = unit_between(points[0], points[1]);
                    line.insert(1, shift(line[0], (ux * jog, uy * jog)));
                }
                line[0] = at_via(points[0], *d);
            }
            if let Some(d) = to_via {
                let last = line.len() - 1;
                if jog > 0.0 && len(line[last - 1], line[last]) > 2.0 * jog {
                    let (ux, uy) = unit_between(points[points.len() - 2], points[points.len() - 1]);
                    line.insert(last, shift(line[last], (-ux * jog, -uy * jog)));
                }
                let last = line.len() - 1;
                line[last] = at_via(points[points.len() - 1], *d);
                if let Some((diameter, drill)) = via {
                    vias.push(Via {
                        at: line[last],
                        drill,
                        diameter,
                        from: LayerId(0),
                        to: LayerId((layers - 1) as u8),
                        net: Some(net),
                    });
                }
            }
            if i == 0 {
                ends_at[k][0] = (line[0], *l);
            }
            if i == runs.len() - 1 {
                ends_at[k][1] = (line[line.len() - 1], *l);
            }
            for w in line.windows(2) {
                if w[0] != w[1] {
                    tracks.push(Track {
                        start: w[0],
                        end: w[1],
                        mid: None,
                        width: geo.widths[usize::from(*l)],
                        layer: LayerId(*l),
                        net: Some(net),
                    });
                }
            }
        }
    }

    // Checked exactly, the positive net's copper first, then the negative's against it.
    // With rip-up, the routes in the way are taken out first.
    let mut undo = Undo::default();
    let allowed = |v: Verdict, rip: &Option<PairRip<'_, '_>>| {
        v == Verdict::Free || (v == Verdict::Routed && rip.is_some())
    };
    for (tracks, vias) in &copper {
        let mut victims = Vec::new();
        for t in tracks {
            let ok = t.net.is_some_and(|net| {
                Shape::track(t.start, t.end, t.width)
                    .is_ok_and(|s| allowed(world.track(net, t.layer, &s, Some(&mut victims)), rip))
            });
            if !ok {
                undo.undo(world);
                return None;
            }
        }
        if let Some(r) = rip.as_mut()
            && !victims.is_empty()
        {
            (r.take)(world, &victims);
        }
        for t in tracks {
            if let (Some(net), Ok(s)) = (t.net, Shape::track(t.start, t.end, t.width)) {
                undo.placed.push(world.add_track(net, t.layer, s));
            }
        }
        for v in vias {
            let mut victims = Vec::new();
            let Some(net) = v.net.filter(|&net| {
                allowed(
                    world.via(net, v.at, v.diameter, v.drill, Some(&mut victims)),
                    rip,
                )
            }) else {
                undo.undo(world);
                return None;
            };
            if let Some(r) = rip.as_mut()
                && !victims.is_empty()
            {
                (r.take)(world, &victims);
            }
            undo.placed
                .push(world.add_via(net, v.at, v.diameter, v.drill));
        }
    }

    // The fan-outs, from each pad to its net's end of the coupled run.
    let mut fanouts: Vec<(usize, NetCopper)> = Vec::new();
    for (end, (i, j)) in [(0, (pa, na)), (1, (pb, nb))] {
        for (k, (ctx, pad)) in [(pc, i), (nc, j)].into_iter().enumerate() {
            let terminal = ctx.pads[pad].terminal;
            let mut pads: Vec<TermPad> = ctx
                .pads
                .iter()
                .filter(|p| p.terminal == terminal)
                .map(|p| TermPad {
                    terminal: 0,
                    ..p.clone()
                })
                .collect();
            let (at, l) = ends_at[k][end];
            pads.push(TermPad {
                center: at,
                layers: LayerSet::EMPTY.with(LayerId(l)),
                bbox: Rect::new(at, at),
                terminal: 1,
            });
            let fan = NetCtx::new(
                ctx.net,
                ctx.widths.clone(),
                ctx.via,
                pads,
                2,
                LATTICE_REACH * params.pitch,
            );
            let steps = StepCache::default();
            let Ok(tree) = detail::route_net(
                world,
                &fan,
                &steps,
                params,
                None,
                rip.as_ref().map(|r| r.rip),
            ) else {
                undo.undo(world);
                return None;
            };
            if let Some(r) = rip.as_mut() {
                let (victims, _) = detail::victims(world, &fan, &tree);
                if !victims.is_empty() {
                    (r.take)(world, &victims);
                }
            }
            let mut c = NetCopper::from_tree(&fan, &tree);
            pull_tight(world, &fan, &mut c);
            c.place(world, &fan);
            fanouts.push((k, c.clone()));
            undo.copper.push(c);
        }
    }
    for (k, c) in fanouts {
        let ctx = if k == 0 { pc } else { nc };
        let (tracks, vias) = &mut copper[k];
        for s in &c.segs {
            tracks.push(Track {
                start: s.a,
                end: s.b,
                mid: None,
                width: ctx.widths[usize::from(s.l)],
                layer: LayerId(s.l),
                net: Some(ctx.net),
            });
        }
        if let Some((diameter, drill)) = ctx.via {
            for &at in &c.vias {
                vias.push(Via {
                    at,
                    drill,
                    diameter,
                    from: LayerId(0),
                    to: LayerId((layers - 1) as u8),
                    net: Some(ctx.net),
                });
            }
        }
    }
    let [(mut tracks, mut vias), (nt, nv)] = copper;
    tracks.extend(nt);
    vias.extend(nv);
    Some(PairRoute {
        tracks,
        vias,
        terminals: [
            [pc.pads[pa].terminal, pc.pads[pb].terminal],
            [nc.pads[na].terminal, nc.pads[nb].terminal],
        ],
        undo,
    })
}
