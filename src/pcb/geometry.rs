//! A net's routes as tracks and vias: turned out of a search tree, kept in the world, and
//! changed by cleanup.

use std::collections::BTreeMap;

use super::detail::{NetCtx, Tree};
use super::world::{Placed, Verdict, World};
use crate::geom::{LayerId, Point, Shape};

/// A straight track of a net.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Seg {
    pub(crate) a: Point,
    pub(crate) b: Point,
    pub(crate) l: u8,
}

impl Seg {
    pub(crate) fn length(&self) -> f64 {
        let dx = (self.b.x - self.a.x) as f64;
        let dy = (self.b.y - self.a.y) as f64;
        (dx * dx + dy * dy).sqrt()
    }
}

/// Whether `p` is on the straight track from `a` to `b`, its ends included.
fn on_seg(p: Point, a: Point, b: Point) -> bool {
    let (ux, uy) = ((b.x - a.x) as i128, (b.y - a.y) as i128);
    let (vx, vy) = ((p.x - a.x) as i128, (p.y - a.y) as i128);
    ux * vy == uy * vx
        && p.x >= a.x.min(b.x)
        && p.x <= a.x.max(b.x)
        && p.y >= a.y.min(b.y)
        && p.y <= a.y.max(b.y)
}

/// The pieces a net's copper leaves it in: by terminal, which piece it is in, and by track
/// and via of the copper, which piece it is part of. The piece of the first terminal is 0,
/// and the others are numbered in the order of the first terminal, and then the first
/// track or via, in them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pieces {
    pub(crate) terminals: Vec<usize>,
    pub(crate) segs: Vec<usize>,
    pub(crate) vias: Vec<usize>,
    pub(crate) count: usize,
}

impl Pieces {
    /// The terminals that are not in the first terminal's piece.
    pub(crate) fn missing(&self) -> Vec<usize> {
        (0..self.terminals.len())
            .filter(|&t| self.terminals[t] != 0)
            .collect()
    }

    /// How many connections the net still needs: the pieces with a terminal, less one.
    pub(crate) fn apart(&self) -> usize {
        let mut with: Vec<usize> = self.terminals.clone();
        with.sort_unstable();
        with.dedup();
        with.len().saturating_sub(1)
    }
}

/// What one net has on the board.
#[derive(Clone, Debug, Default)]
pub(crate) struct NetCopper {
    pub(crate) segs: Vec<Seg>,
    pub(crate) vias: Vec<Point>,
    placed: Vec<Placed>,
}

impl NetCopper {
    /// The tracks and vias of a tree, with straight runs merged.
    pub(crate) fn from_tree(ctx: &NetCtx, tree: &Tree) -> NetCopper {
        let mut segs = Vec::new();
        let mut vias: Vec<Point> = Vec::new();
        for &(a, b) in &tree.edges {
            if a.l == b.l {
                if a.point() != b.point() {
                    segs.push(Seg {
                        a: a.point(),
                        b: b.point(),
                        l: a.l,
                    });
                }
            } else {
                let p = a.point();
                if !ctx.joins(p, a.l, b.l) && !vias.contains(&p) {
                    vias.push(p);
                }
            }
        }
        let mut copper = NetCopper {
            segs,
            vias,
            placed: Vec::new(),
        };
        copper.merge(ctx);
        copper
    }

    /// Points where tracks must keep an end: pads' centres and vias.
    fn anchors(&self, ctx: &NetCtx) -> Vec<Point> {
        let mut out: Vec<Point> = ctx.pads.iter().map(|p| p.center).collect();
        out.extend(self.vias.iter().copied());
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The tracks at each point of each layer, by position in `segs`.
    pub(crate) fn incidence(&self) -> BTreeMap<(u8, Point), Vec<usize>> {
        let mut map: BTreeMap<(u8, Point), Vec<usize>> = BTreeMap::new();
        for (i, s) in self.segs.iter().enumerate() {
            map.entry((s.l, s.a)).or_default().push(i);
            map.entry((s.l, s.b)).or_default().push(i);
        }
        map
    }

    /// Joins tracks that continue each other in a straight line, where nothing else meets
    /// them, and drops tracks that repeat others.
    pub(crate) fn merge(&mut self, ctx: &NetCtx) {
        self.segs.retain(|s| s.a != s.b);
        for s in &mut self.segs {
            if s.b < s.a {
                core::mem::swap(&mut s.a, &mut s.b);
            }
        }
        self.segs.sort_unstable();
        self.segs.dedup();
        let anchors = self.anchors(ctx);
        let mut live: Vec<Option<Seg>> = self.segs.iter().copied().map(Some).collect();
        let mut map = self.incidence();
        let keys: Vec<(u8, Point)> = map.keys().copied().collect();
        for key in keys {
            let (l, p) = key;
            if anchors.binary_search(&p).is_ok() {
                continue;
            }
            let Some(at) = map.get(&key) else { continue };
            if at.len() != 2 {
                continue;
            }
            let (i, j) = (at[0], at[1]);
            let (Some(s), Some(t)) = (live[i], live[j]) else {
                continue;
            };
            let far = |s: Seg| if s.a == p { s.b } else { s.a };
            let (a, b) = (far(s), far(t));
            // Collinear and on opposite sides of `p`.
            let (ux, uy) = (a.x - p.x, a.y - p.y);
            let (vx, vy) = (b.x - p.x, b.y - p.y);
            if (ux as i128) * (vy as i128) != (uy as i128) * (vx as i128)
                || (ux as i128) * (vx as i128) + (uy as i128) * (vy as i128) >= 0
            {
                continue;
            }
            live[i] = Some(Seg { a, b, l });
            live[j] = None;
            map.remove(&key);
            if let Some(v) = map.get_mut(&(l, b)) {
                for k in v.iter_mut() {
                    if *k == j {
                        *k = i;
                    }
                }
            }
        }
        self.segs = live.into_iter().flatten().collect();
    }

    /// The pieces this copper leaves the net in: tracks are joined where an end of one is
    /// on another, on the same layer, vias join the layers where they are, and a pad joins
    /// the tracks that end at its centre to the other pads of its terminal.
    pub(crate) fn pieces(&self, ctx: &NetCtx) -> Pieces {
        let (ns, nv, nt) = (self.segs.len(), self.vias.len(), ctx.terminals);
        // Tracks, then vias, then terminals.
        let mut parent: Vec<usize> = (0..ns + nv + nt).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        let mut join = |a: usize, b: usize| {
            let (x, y) = (find(&mut parent, a), find(&mut parent, b));
            if x != y {
                parent[x.max(y)] = x.min(y);
            }
        };
        // By layer, the tracks in order of their boxes' left sides, so that each is only
        // looked at against those that may reach it.
        let mut order: Vec<usize> = (0..ns).collect();
        order.sort_by_key(|&i| (self.segs[i].l, self.segs[i].a.x.min(self.segs[i].b.x), i));
        for (k, &i) in order.iter().enumerate() {
            let s = self.segs[i];
            let right = s.a.x.max(s.b.x);
            for &j in &order[k + 1..] {
                let t = self.segs[j];
                if t.l != s.l || t.a.x.min(t.b.x) > right {
                    break;
                }
                if on_seg(t.a, s.a, s.b)
                    || on_seg(t.b, s.a, s.b)
                    || on_seg(s.a, t.a, t.b)
                    || on_seg(s.b, t.a, t.b)
                {
                    join(i, j);
                }
            }
        }
        for (v, &at) in self.vias.iter().enumerate() {
            for (i, s) in self.segs.iter().enumerate() {
                if on_seg(at, s.a, s.b) {
                    join(ns + v, i);
                }
            }
        }
        for p in &ctx.pads {
            let t = ns + nv + p.terminal;
            for (i, s) in self.segs.iter().enumerate() {
                if p.layers.contains(LayerId(s.l)) && (s.a == p.center || s.b == p.center) {
                    join(t, i);
                }
            }
            for (v, &at) in self.vias.iter().enumerate() {
                if at == p.center {
                    join(t, ns + v);
                }
            }
        }
        // Numbered from the first terminal's.
        let mut number: BTreeMap<usize, usize> = BTreeMap::new();
        let mut piece = |i: usize, parent: &mut [usize]| {
            let root = find(parent, i);
            let next = number.len();
            *number.entry(root).or_insert(next)
        };
        let terminals: Vec<usize> = (0..nt).map(|t| piece(ns + nv + t, &mut parent)).collect();
        let segs: Vec<usize> = (0..ns).map(|i| piece(i, &mut parent)).collect();
        let vias: Vec<usize> = (0..nv).map(|v| piece(ns + v, &mut parent)).collect();
        Pieces {
            terminals,
            segs,
            vias,
            count: number.len(),
        }
    }

    /// What of this copper, which is not in the world, may be put back into it as it is
    /// now: the tracks and vias that nothing there is in the way of. A track with
    /// something in its way is looked at in steps of `pitch`, and keeps those that are
    /// free, so that the net is cut only where it has to be. What is left may be in
    /// pieces, and have ends that lead nowhere: they are where to join it up again from.
    pub(crate) fn free_in(&self, world: &World<'_>, ctx: &NetCtx, pitch: i64) -> NetCopper {
        let free = |a: Point, b: Point, l: u8| {
            Shape::track(a, b, ctx.widths[usize::from(l)])
                .is_ok_and(|s| world.track(ctx.net, LayerId(l), &s, None) == Verdict::Free)
        };
        let mut segs = Vec::new();
        for s in &self.segs {
            if free(s.a, s.b, s.l) {
                segs.push(*s);
                continue;
            }
            let (dx, dy) = (s.b.x - s.a.x, s.b.y - s.a.y);
            let n = dx.abs().max(dy.abs()) / pitch.max(1);
            // Only a track along one of the eight directions has steps to look at.
            if n < 2 || (dx != 0 && dy != 0 && dx.abs() != dy.abs()) {
                continue;
            }
            let (ux, uy) = (dx.signum() * pitch, dy.signum() * pitch);
            let at = |k: i64| {
                if k == n {
                    s.b
                } else {
                    Point::new(s.a.x + k * ux, s.a.y + k * uy)
                }
            };
            for k in 0..n {
                if free(at(k), at(k + 1), s.l) {
                    segs.push(Seg {
                        a: at(k),
                        b: at(k + 1),
                        l: s.l,
                    });
                }
            }
        }
        let vias = match ctx.via {
            Some((d, drill)) => self
                .vias
                .iter()
                .copied()
                .filter(|&v| world.via(ctx.net, v, d, drill, None) == Verdict::Free)
                .collect(),
            None => Vec::new(),
        };
        let mut kept = NetCopper {
            segs,
            vias,
            placed: Vec::new(),
        };
        kept.merge(ctx);
        kept
    }

    /// Drops the pieces of copper with no terminal in them.
    pub(crate) fn drop_floating(&mut self, ctx: &NetCtx) {
        let pieces = self.pieces(ctx);
        let mut live = vec![false; pieces.count];
        for &p in &pieces.terminals {
            live[p] = true;
        }
        let mut k = 0;
        self.segs.retain(|_| {
            k += 1;
            live[pieces.segs[k - 1]]
        });
        let mut k = 0;
        self.vias.retain(|_| {
            k += 1;
            live[pieces.vias[k - 1]]
        });
    }

    /// Cuts the tracks where another ends on them, or a via or a pad's centre is, so that
    /// tracks only meet at their ends.
    fn split_at_joints(&mut self, ctx: &NetCtx) {
        let mut out: Vec<Seg> = Vec::with_capacity(self.segs.len());
        for (i, s) in self.segs.iter().enumerate() {
            let inside = |p: Point| p != s.a && p != s.b && on_seg(p, s.a, s.b);
            let mut cuts: Vec<Point> = Vec::new();
            for (j, t) in self.segs.iter().enumerate() {
                if j != i && t.l == s.l {
                    cuts.extend([t.a, t.b].into_iter().filter(|&p| inside(p)));
                }
            }
            cuts.extend(self.vias.iter().copied().filter(|&p| inside(p)));
            cuts.extend(
                ctx.pads
                    .iter()
                    .filter(|p| p.layers.contains(LayerId(s.l)))
                    .map(|p| p.center)
                    .filter(|&p| inside(p)),
            );
            if cuts.is_empty() {
                out.push(*s);
                continue;
            }
            let far = |p: Point| (p.x - s.a.x).abs().max((p.y - s.a.y).abs());
            cuts.sort_by_key(|&p| far(p));
            cuts.dedup();
            let mut from = s.a;
            for p in cuts.into_iter().chain([s.b]) {
                out.push(Seg {
                    a: from,
                    b: p,
                    l: s.l,
                });
                from = p;
            }
        }
        self.segs = out;
        self.segs.sort_unstable();
        self.segs.dedup();
    }

    /// Drops the copper that joins nothing: pieces with no terminal in them, tracks that
    /// end where there is neither a pad, a via nor another track, one after the other,
    /// and vias with copper on fewer than two layers.
    pub(crate) fn prune(&mut self, ctx: &NetCtx) {
        self.drop_floating(ctx);
        self.split_at_joints(ctx);
        loop {
            let before = (self.segs.len(), self.vias.len());
            // How many tracks end at each point of each layer.
            let ends = self.incidence();
            let held = |l: u8, p: Point| {
                ends.get(&(l, p)).is_some_and(|v| v.len() > 1)
                    || self.vias.contains(&p)
                    || ctx
                        .pads
                        .iter()
                        .any(|q| q.center == p && q.layers.contains(LayerId(l)))
            };
            let dead: Vec<bool> = self
                .segs
                .iter()
                .map(|s| !held(s.l, s.a) || !held(s.l, s.b))
                .collect();
            let mut k = 0;
            self.segs.retain(|_| {
                k += 1;
                !dead[k - 1]
            });
            let segs = &self.segs;
            self.vias.retain(|&v| {
                let mut layers = ctx
                    .pads
                    .iter()
                    .filter(|p| p.center == v)
                    .fold(0u64, |m, p| m | p.layers.0);
                for s in segs.iter().filter(|s| s.a == v || s.b == v) {
                    layers |= 1 << s.l;
                }
                layers.count_ones() > 1
            });
            if (self.segs.len(), self.vias.len()) == before {
                break;
            }
        }
        self.merge(ctx);
    }

    /// The same tracks and vias, as copper that is not in the world.
    pub(crate) fn detached(&self) -> NetCopper {
        NetCopper {
            segs: self.segs.clone(),
            vias: self.vias.clone(),
            placed: Vec::new(),
        }
    }

    /// Whether there is no copper.
    pub(crate) fn is_empty(&self) -> bool {
        self.segs.is_empty() && self.vias.is_empty()
    }

    /// The total length of the tracks, in nanometres.
    pub(crate) fn length(&self) -> f64 {
        self.segs.iter().map(Seg::length).sum()
    }

    /// Puts the copper into the world.
    pub(crate) fn place(&mut self, world: &mut World<'_>, ctx: &NetCtx) {
        self.unplace(world);
        for s in &self.segs {
            if let Ok(shape) = Shape::track(s.a, s.b, ctx.widths[usize::from(s.l)]) {
                self.placed
                    .push(world.add_track(ctx.net, LayerId(s.l), shape));
            }
        }
        if let Some((d, drill)) = ctx.via {
            for &v in &self.vias {
                self.placed.push(world.add_via(ctx.net, v, d, drill));
            }
        }
    }

    /// Takes the copper out of the world again.
    pub(crate) fn unplace(&mut self, world: &mut World<'_>) {
        for p in self.placed.drain(..) {
            world.remove(&p);
        }
    }
}
