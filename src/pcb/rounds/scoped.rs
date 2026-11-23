//! Scoped clearance rules for new copper: the clearance the DRC computes between two items
//! (`crate::board::clearance`), applied to tracks and vias that do not exist yet.
//!
//! The DRC resolves the clearance between two items from the custom rules holding them (the
//! last one in board order wins, rules between two scopes included), else their local
//! clearances, else their net classes, never under the board's `min_clearance`. For existing
//! copper (pads, tracks, vias) the rules holding it are known exactly. For new copper they
//! depend on where it is: a rule scoped to a courtyard or an area holds a track that reaches
//! into it (or lies entirely inside it), on its layer, of its kind. Every rule scope that can
//! hold new copper becomes a *condition*, and new copper has a value per condition: surely
//! held, surely not, or maybe (near the boundary: within the copper's own extent plus a margin,
//! or within the uncertainty of where the copper is). A state [`St`] is the set of conditions
//! surely held and the set maybe held; the clearance used is the largest the DRC could compute
//! for any combination of the maybes, so it is never less than the DRC's.
//!
//! The DRC decides per item, and a track crossing a boundary is decided as a whole; routed
//! tracks are therefore split where their state changes ([`Scoped::split`]), so the DRC sees
//! tracks held uniformly and decides what the router assumed at every point.
//!
//! Plated holes keep the board's `hole_clearance` from copper of other nets (D56): a via
//! reaches the larger of its pad plus the clearance and its drill plus the hole clearance
//! ([`Scoped::via_out`]).
//!
//! Without custom clearance rules, local clearances or a hole clearance nothing here changes
//! the router's values: every state is empty and the clearance between two nets is the larger
//! of their classes, as before.

use crate::geom::{LayerSet, Rules, Shape as GShape};
use crate::id::NetId;

use super::geo::{BoxF, P, Poly, point_seg_d2};

/// Margin (nm) by which a point must be inside or outside a boundary to be surely held or not:
/// covers the rounding of split points to nanometers and the DRC's own measurement.
const EPS: f64 = 1_000.0;
/// Margin (nm) for "entirely inside an area": the DRC tests the polygon of the copper, whose
/// round ends are approximated outwards by up to 1 µm.
const EPS_INSIDE: f64 = 5_000.0;
/// Pieces of a split are found to this precision (nm).
const SPLIT_RES: f64 = 20.0;
/// Most conditions with their own state bit (more are always "maybe").
const MAX_BITS: usize = 64;
/// Most candidate rules per side before a clearance falls back to the largest any rule or class
/// gives.
const MAX_CANDS: usize = 16;

/// The state of new copper: conditions surely held (`sure`) and possibly held (`maybe`, a
/// superset). The default (nothing held) is the state everywhere on boards without positional
/// rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct St {
    pub sure: u64,
    pub maybe: u64,
}

impl St {
    /// A state covering both.
    pub fn union(self, o: St) -> St {
        St {
            sure: self.sure & o.sure,
            maybe: self.maybe | o.maybe,
        }
    }
}

/// Kind of new copper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Track,
    Via,
}

/// A condition's value over a disk of points: all held, none, all in the band between, or a
/// mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tri {
    In,
    Out,
    Maybe,
    Mixed,
}

/// Value of a distance range `[lo, hi]`: held below `t_in`, not from `t_out`, maybe between.
fn tri(lo: f64, hi: f64, t_in: f64, t_out: f64) -> Tri {
    if hi < t_in {
        Tri::In
    } else if lo >= t_out {
        Tri::Out
    } else if lo >= t_in && hi < t_out {
        Tri::Maybe
    } else {
        Tri::Mixed
    }
}

/// Any of the values holds.
fn any(v: impl Iterator<Item = Tri>) -> Tri {
    let (mut all_out, mut mixed) = (true, false);
    for t in v {
        match t {
            Tri::In => return Tri::In,
            Tri::Out => {}
            Tri::Maybe => all_out = false,
            Tri::Mixed => {
                all_out = false;
                mixed = true;
            }
        }
    }
    if all_out {
        Tri::Out
    } else if mixed {
        Tri::Mixed
    } else {
        Tri::Maybe
    }
}

/// All of the values hold.
fn all(v: impl Iterator<Item = Tri>) -> Tri {
    let (mut maybe, mut mixed) = (false, false);
    for t in v {
        match t {
            Tri::Out => return Tri::Out,
            Tri::In => {}
            Tri::Maybe => maybe = true,
            Tri::Mixed => mixed = true,
        }
    }
    if mixed {
        Tri::Mixed
    } else if maybe {
        Tri::Maybe
    } else {
        Tri::In
    }
}

/// Signed distance from `p` to a simple polygon: negative inside.
fn sdist(poly: &Poly, p: P) -> f64 {
    let mut d2 = f64::MAX;
    for (a, b) in poly.edges() {
        d2 = d2.min(point_seg_d2(p, a, b));
    }
    let d = d2.sqrt();
    if poly.contains(p) { -d } else { d }
}

/// A condition: the terms of one rule scope (or `with` scope) for new copper.
struct Cond {
    /// Tracks: on the stackup layers of this mask.
    track: u64,
    /// Vias (through: on every layer).
    via: bool,
    /// Reached into: every group must hold, one polygon of a group is enough.
    touch: Vec<Vec<Poly>>,
    /// Entirely inside one polygon of each group.
    inside: Vec<Vec<Poly>>,
    /// Box of every polygon.
    bbox: BoxF,
}

impl Cond {
    /// The condition over the centerline points within `r` of `p`, for copper reaching `hw`
    /// beyond its centerline.
    fn eval(&self, kind: Kind, layer: usize, p: P, r: f64, hw: f64) -> Tri {
        let gate = match kind {
            Kind::Track => self.track >> layer.min(63) & 1 == 1,
            Kind::Via => self.via,
        };
        if !gate {
            return Tri::Out;
        }
        if self.touch.is_empty() && self.inside.is_empty() {
            return Tri::In;
        }
        // Far from every polygon: out of all of them.
        if self.bbox.dist(p) >= r + hw + EPS_INSIDE + EPS {
            return Tri::Out;
        }
        let touch = |g: &Vec<Poly>| {
            any(g.iter().map(|poly| {
                if poly.bbox.dist(p) >= r + hw + EPS {
                    return Tri::Out;
                }
                let d = sdist(poly, p);
                tri(d - r, d + r, -EPS, hw + EPS)
            }))
        };
        let inside = |g: &Vec<Poly>| {
            any(g.iter().map(|poly| {
                if poly.bbox.dist(p) >= r + EPS {
                    return Tri::Out;
                }
                let d = sdist(poly, p);
                tri(d - r, d + r, -(hw + EPS_INSIDE), EPS)
            }))
        };
        all(self
            .touch
            .iter()
            .map(touch)
            .chain(self.inside.iter().map(inside)))
    }
}

/// Where a rule's side stands for new copper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// Its condition bit.
    Bit(u8),
    /// No bit left: always maybe.
    Maybe,
}

impl Side {
    /// (surely, maybe) held in state `st`.
    fn holds(self, st: St) -> (bool, bool) {
        match self {
            Side::Bit(b) => (st.sure >> b & 1 == 1, st.maybe >> b & 1 == 1),
            Side::Maybe => (false, true),
        }
    }
}

/// What the router knows of an existing item's rules (exact).
#[derive(Clone, Debug, Default)]
struct ItemRule {
    /// The last custom rule without `with` holding it.
    single: Option<u16>,
    class: f64,
    local: Option<f64>,
    /// Rules between two scopes whose `scope` / `with` hold it.
    a: Vec<u16>,
    b: Vec<u16>,
}

/// One side of a clearance.
#[derive(Clone, Copy)]
pub(crate) enum Who {
    /// New copper: its state and net (class) clearance.
    New(St, f64),
    /// The existing copper item of this index.
    Item(u32),
}

/// The scoped clearance rules of a board for the router (see the module docs).
pub(crate) struct Scoped {
    /// Whether anything beyond net classes applies to clearances (custom clearance rules or
    /// local clearances); when not, every clearance is the larger of the two classes.
    pub on: bool,
    conds: Vec<Cond>,
    /// Rules without `with` that may hold new copper, in board order: (rule index, side).
    singles: Vec<(u16, Side)>,
    /// Rules between two scopes: (rule index, `scope` side, `with` side), board order.
    paired: Vec<(u16, Side, Side)>,
    /// Clearance per rule index (0 without one).
    value: Vec<f64>,
    /// Largest rule clearance.
    max_value: f64,
    /// Largest class or local clearance of an existing item.
    max_item: f64,
    floor: f64,
    items: Vec<ItemRule>,
    /// Copper-to-hole clearance of plated holes (0: none).
    pub hc: f64,
    /// Track width rules that may hold new tracks: (side, width), board order.
    widths: Vec<(Side, f64)>,
}

impl Scoped {
    /// Prepares the rules of a board for `items` (every copper item, with the layers it is
    /// on, its net and the clearance of its own if it has one; existing copper gets its exact
    /// rules).
    ///
    /// The board's rules have two things beyond the nets' classes: a clearance of an item's
    /// own, which is the local clearance here, and areas with a clearance between two pieces
    /// of copper that both touch them, the first that both touch in place of any other. An
    /// area is a rule between its scope and itself, and the rules are in the reverse of the
    /// areas' order, so that the last one holding both sides is the first area.
    pub fn new(rules: &Rules, items: &[(&GShape, LayerSet, Option<NetId>, Option<i64>)]) -> Scoped {
        let spec = rules.spec();
        let floor = spec.min_clearance as f64;
        let areas: Vec<&crate::geom::RuleArea> = spec
            .areas
            .iter()
            .rev()
            .filter(|a| a.overrides.clearance.is_some())
            .collect();
        let any_local = items.iter().any(|it| it.3.is_some());
        let value: Vec<f64> = areas
            .iter()
            .map(|a| a.overrides.clearance.unwrap_or(0) as f64)
            .collect();
        let mut out = Scoped {
            on: !areas.is_empty() || any_local,
            conds: Vec::new(),
            singles: Vec::new(),
            paired: Vec::new(),
            max_value: value.iter().copied().fold(0.0, f64::max),
            value,
            max_item: 0.0,
            floor,
            items: Vec::new(),
            hc: spec.hole_clearance.max(0) as f64,
            widths: Vec::new(),
        };
        for (k, a) in areas.iter().enumerate() {
            // No bit left: always maybe.
            if out.conds.len() >= MAX_BITS {
                out.paired.push((k as u16, Side::Maybe, Side::Maybe));
                continue;
            }
            let polys: Vec<Poly> = match super::geo::Shape::of(&a.shape) {
                super::geo::Shape::Polys(v) => v,
                super::geo::Shape::Capsule { .. } => match a.shape.outline(1_000) {
                    Ok(pg) => pg
                        .iter()
                        .map(|rings| {
                            Poly::new(
                                rings
                                    .iter()
                                    .map(|r| r.iter().map(|q| P::of(*q)).collect())
                                    .collect(),
                            )
                        })
                        .collect(),
                    Err(_) => Vec::new(),
                },
            };
            let mut bbox = BoxF::EMPTY;
            for poly in &polys {
                bbox = bbox.union(poly.bbox);
            }
            // A via is copper on every layer: an area holds it where it is on them all.
            let all = rules.layers().0;
            out.conds.push(Cond {
                track: a.layers.0,
                via: a.layers.0 & all == all,
                touch: vec![polys],
                inside: Vec::new(),
                bbox,
            });
            let side = Side::Bit((out.conds.len() - 1) as u8);
            out.paired.push((k as u16, side, side));
        }
        if out.on {
            let first = |l: LayerSet| l.iter().next().unwrap_or(crate::geom::LayerId(0));
            out.items = items
                .iter()
                .map(|&(shape, layers, net, own)| {
                    let held: Vec<u16> = areas
                        .iter()
                        .enumerate()
                        .filter(|(_, a)| {
                            a.layers.0 & layers.0 != 0
                                && a.shape.bbox().intersects(&shape.bbox())
                                && a.shape.touches(shape)
                        })
                        .map(|(k, _)| k as u16)
                        .collect();
                    ItemRule {
                        single: None,
                        class: rules.clearance(net, first(layers)) as f64,
                        local: own.map(|v| v as f64),
                        a: held.clone(),
                        b: held,
                    }
                })
                .collect();
            out.max_item = out
                .items
                .iter()
                .map(|it| it.class.max(it.local.unwrap_or(0.0)))
                .fold(0.0, f64::max);
        }
        out
    }

    /// Whether new copper has positional states at all (conditions exist).
    pub fn positional(&self) -> bool {
        !self.conds.is_empty()
    }

    /// The state of new copper of `kind` on stackup layer `layer` (ignored for vias) whose
    /// centerline points lie within `r` of `p`, reaching `hw` beyond them (track half width,
    /// via radius).
    pub fn st(&self, kind: Kind, layer: usize, p: P, r: f64, hw: f64) -> St {
        let mut st = St::default();
        for (b, c) in self.conds.iter().enumerate() {
            match c.eval(kind, layer, p, r, hw) {
                Tri::In => {
                    st.sure |= 1 << b;
                    st.maybe |= 1 << b;
                }
                Tri::Out => {}
                Tri::Maybe | Tri::Mixed => st.maybe |= 1 << b,
            }
        }
        st
    }

    /// The state of tracks anywhere in `bbox` on any layer of `layers` (all when empty).
    pub fn st_box(&self, kind: Kind, layers: &[usize], bbox: &BoxF, hw: f64) -> St {
        if self.conds.is_empty() || bbox.is_empty() {
            return St::default();
        }
        let c = P::new(
            (bbox.min.x + bbox.max.x) / 2.0,
            (bbox.min.y + bbox.max.y) / 2.0,
        );
        let r = c.dist(bbox.max);
        match kind {
            Kind::Via => self.st(Kind::Via, 0, c, r, hw),
            Kind::Track => {
                let mut out: Option<St> = None;
                for &l in layers {
                    let s = self.st(Kind::Track, l, c, r, hw);
                    out = Some(out.map_or(s, |o| o.union(s)));
                }
                out.unwrap_or(St {
                    sure: 0,
                    maybe: u64::MAX,
                })
            }
        }
    }

    /// Pieces of a straight track on `layer` from `a` to `b` (half width `hw`, centerline points
    /// within `inner` of the segment), each with the state of its points: (from, to, state),
    /// from `a` to `b`. Pieces are uniform: no condition changes inside one (to within
    /// nanometers at its ends).
    pub fn split(&self, layer: usize, a: P, b: P, inner: f64, hw: f64) -> Vec<(P, P, St)> {
        if self.conds.is_empty() || a == b {
            let st = self.st(Kind::Track, layer, a, inner, hw);
            return vec![(a, b, st)];
        }
        let len = a.dist(b);
        let at = |t: f64| P::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        // Leaves: (t0, t1, state of every point). A leaf where a condition changes is shorter
        // than the resolution; its state covers both sides, so it is the "maybe" state of the
        // band next to it and merges with it.
        let mut leaves: Vec<(f64, f64, St)> = Vec::new();
        let mut stack: Vec<(f64, f64)> = vec![(0.0, 1.0)];
        while let Some((t0, t1)) = stack.pop() {
            let m = at((t0 + t1) / 2.0);
            let half = len * (t1 - t0) / 2.0;
            let mut uniform = true;
            let mut st = St::default();
            for (bit, c) in self.conds.iter().enumerate() {
                match c.eval(Kind::Track, layer, m, half + inner, hw) {
                    Tri::In => {
                        st.sure |= 1 << bit;
                        st.maybe |= 1 << bit;
                    }
                    Tri::Out => {}
                    Tri::Maybe => st.maybe |= 1 << bit,
                    Tri::Mixed => {
                        uniform = false;
                        st.maybe |= 1 << bit;
                    }
                }
            }
            if uniform || half * 2.0 <= SPLIT_RES {
                leaves.push((t0, t1, st));
            } else {
                // Second half first: leaves come out in order from `a`.
                let tm = (t0 + t1) / 2.0;
                stack.push((tm, t1));
                stack.push((t0, tm));
            }
        }
        let mut out: Vec<(f64, f64, St)> = Vec::new();
        for (t0, t1, st) in leaves {
            match out.last_mut() {
                Some(last) if last.2 == st => last.1 = t1,
                _ => out.push((t0, t1, st)),
            }
        }
        out.into_iter()
            .map(|(t0, t1, st)| {
                (
                    if t0 == 0.0 { a } else { at(t0) },
                    if t1 == 1.0 { b } else { at(t1) },
                    st,
                )
            })
            .collect()
    }

    /// Every single rule that can be the last one holding `w` (`None`: no rule) into `out`;
    /// their number, or `None` when there are too many to list.
    fn singles_of(&self, w: Who, out: &mut [Option<u16>; MAX_CANDS]) -> Option<usize> {
        match w {
            Who::Item(i) => {
                out[0] = self.items[i as usize].single;
                Some(1)
            }
            Who::New(st, _) => {
                let mut n = 0;
                for &(k, s) in self.singles.iter().rev() {
                    let (sure, maybe) = s.holds(st);
                    if maybe {
                        if n == MAX_CANDS {
                            return None;
                        }
                        out[n] = Some(k);
                        n += 1;
                    }
                    if sure {
                        return Some(n);
                    }
                }
                if n == MAX_CANDS {
                    return None;
                }
                out[n] = None;
                Some(n + 1)
            }
        }
    }

    /// (surely, maybe) the `scope` / `with` side of paired rule `pi` holds `w`.
    fn pair_side(&self, w: Who, pi: usize, with: bool) -> (bool, bool) {
        let (k, sa, sb) = self.paired[pi];
        match w {
            Who::Item(i) => {
                let it = &self.items[i as usize];
                let v = if with {
                    it.b.contains(&k)
                } else {
                    it.a.contains(&k)
                };
                (v, v)
            }
            Who::New(st, _) => (if with { sb } else { sa }).holds(st),
        }
    }

    /// Class and local clearance of a side.
    fn base(&self, w: Who) -> (f64, Option<f64>) {
        match w {
            Who::Item(i) => (self.items[i as usize].class, self.items[i as usize].local),
            Who::New(_, c) => (c, None),
        }
    }

    /// The clearance the DRC requires between `x` and `y` (of different nets), the largest
    /// over what new copper's states leave open.
    pub fn resolve(&self, x: Who, y: Who) -> f64 {
        debug_assert!(self.on, "resolve without rules");
        let ((cx, lx), (cy, ly)) = (self.base(x), self.base(y));
        let fallback = match (lx, ly) {
            (None, None) => cx.max(cy),
            (a, b) => a.unwrap_or(0.0).max(b.unwrap_or(0.0)),
        };
        let mut sa = [None; MAX_CANDS];
        let mut sb = [None; MAX_CANDS];
        let (Some(na), Some(nb)) = (self.singles_of(x, &mut sa), self.singles_of(y, &mut sb))
        else {
            return self
                .max_value
                .max(cx)
                .max(cy)
                .max(lx.unwrap_or(0.0))
                .max(ly.unwrap_or(0.0))
                .max(self.floor);
        };
        // Paired rules: the last surely holding, and those maybe holding.
        let mut sure_pair: Option<u16> = None;
        let mut maybe_pairs: [u16; MAX_CANDS] = [0; MAX_CANDS];
        let mut nm = 0;
        for pi in 0..self.paired.len() {
            let (xa, xb, ya, yb) = (
                self.pair_side(x, pi, false),
                self.pair_side(x, pi, true),
                self.pair_side(y, pi, false),
                self.pair_side(y, pi, true),
            );
            let sure = (xa.0 && yb.0) || (xb.0 && ya.0);
            let maybe = (xa.1 && yb.1) || (xb.1 && ya.1);
            let k = self.paired[pi].0;
            if sure {
                sure_pair = Some(k);
            } else if maybe {
                if nm == MAX_CANDS {
                    return self
                        .max_value
                        .max(cx)
                        .max(cy)
                        .max(lx.unwrap_or(0.0))
                        .max(ly.unwrap_or(0.0))
                        .max(self.floor);
                }
                maybe_pairs[nm] = k;
                nm += 1;
            }
        }
        let value = |k: Option<u16>| k.map_or(fallback, |k| self.value[k as usize]);
        let mut best = 0.0f64;
        for &a in &sa[..na] {
            for &b in &sb[..nb] {
                let win = a.max(b).max(sure_pair);
                best = best.max(value(win));
                for &k in &maybe_pairs[..nm] {
                    if Some(k) > win {
                        best = best.max(self.value[k as usize]);
                    }
                }
            }
        }
        best.max(self.floor)
    }

    /// The distance from a via's center to copper of another net it must keep, at copper
    /// clearance `c`: its pad plus `c`, and its drill plus the hole clearance.
    pub fn via_out(&self, rv: f64, dr: f64, c: f64) -> f64 {
        (rv + c).max(dr + self.hc)
    }

    /// The distance between the centers of two vias of different nets at copper clearance `c`
    /// (hole-to-hole `h2h` included).
    pub fn via_via(&self, (rv1, dr1): (f64, f64), (rv2, dr2): (f64, f64), c: f64, h2h: f64) -> f64 {
        (rv1 + rv2 + c)
            .max(dr1 + rv2 + self.hc)
            .max(rv1 + dr2 + self.hc)
            .max(dr1 + dr2 + h2h)
    }

    /// The largest track width rule that may hold a track in state `st` (0 when none).
    pub fn min_width(&self, st: St) -> f64 {
        // The last rule holding a track decides; any of the maybes may be it.
        let mut best = 0.0f64;
        for &(s, w) in self.widths.iter().rev() {
            let (sure, maybe) = s.holds(st);
            if maybe {
                best = best.max(w);
            }
            if sure {
                break;
            }
        }
        best
    }

    /// The largest clearance a rule or the board minimum can ask for.
    pub fn max_clear(&self) -> f64 {
        self.max_value.max(self.floor).max(self.max_item)
    }

    /// Whether track width rules exist for new tracks.
    pub fn has_widths(&self) -> bool {
        !self.widths.is_empty()
    }
}
