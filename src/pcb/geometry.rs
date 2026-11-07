//! A net's routes as tracks and vias: turned out of a search tree, kept in the world, and
//! changed by cleanup.

use std::collections::BTreeMap;

use super::detail::{NetCtx, Tree};
use super::world::{Placed, World};
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
