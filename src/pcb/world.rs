//! Everything routes must keep clear of, indexed by layer, with the exact questions the
//! router asks of it: may a track or a via of a net go here, and if not, whose copper is in
//! the way?
//!
//! The answers follow [`drc::check`] rule for rule, so that what is legal here passes the
//! check: clearances between nets, with those of pairs of nets and of items of their own,
//! hole clearances and hole-to-hole clearances, keepouts, and, which the check doesn't
//! have, the clearance to the board's edge.

use core::cell::RefCell;

use super::MaskOpening;
use crate::geom::drc::{self, Board};
use crate::geom::{EntryId, GeomError, LayerId, LayerSet, Point, Rules, Shape, ShapeIndex};
use crate::id::NetId;

/// What an entry of the index is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Copper.
    Copper,
    /// A drilled hole, which goes through every layer.
    Hole,
    /// A keepout, with whether it keeps out tracks and vias.
    Keepout { tracks: bool, vias: bool },
    /// The board's edge.
    Edge,
    /// An opening in the solder mask, reaching this far beyond its shape.
    Mask { margin: i64 },
}

/// Who put an entry there: the design, or a route of the router's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Owner {
    Fixed,
    Routed(NetId),
}

#[derive(Clone, Copy, Debug)]
struct Meta {
    kind: Kind,
    net: Option<NetId>,
    own: Option<i64>,
    owner: Owner,
    /// The clearance of its net on its layer, as [`Rules::clearance`] gives it, so that
    /// queries need not resolve it again.
    clearance: i64,
}

/// A handle on what one routed item added, to take it out again.
#[derive(Clone, Debug, Default)]
pub(crate) struct Placed {
    entries: Vec<(u8, EntryId)>,
}

/// The answer to "may this go here?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Nothing is in the way.
    Free,
    /// Only routes of other nets are, which a rip-up could move.
    Routed,
    /// Something fixed is.
    Blocked,
}

/// The obstacles, by layer.
pub(crate) struct World<'r> {
    rules: &'r Rules,
    edge_clearance: i64,
    layers: Vec<ShapeIndex>,
    meta: Vec<Vec<Meta>>,
    /// How far any rule can reach, for widening queries.
    reach: i64,
    scratch: RefCell<Vec<EntryId>>,
    /// By net, whether its routes may no longer be ripped up, so that they block like
    /// fixed copper.
    locked: Vec<bool>,
    /// By net and then layer, [`Rules::clearance`], for the nets of the board's items,
    /// which costs more to resolve than many a query.
    clearances: Vec<i64>,
}

impl<'r> World<'r> {
    /// The fixed items of a board, and its edge.
    pub(crate) fn new(
        board: &Board,
        rules: &'r Rules,
        outline: &[Shape],
        edge_clearance: i64,
        masks: &[MaskOpening],
        obstacles: &[(Shape, LayerId)],
    ) -> Result<World<'r>, GeomError> {
        let n = rules.layer_count();
        let own_max = board
            .pads
            .iter()
            .filter_map(|p| p.clearance)
            .chain(board.zones.iter().filter_map(|z| z.clearance))
            .chain(masks.iter().map(|m| m.margin + MASK_GUARD))
            .fold(0, i64::max);
        let reach = rules
            .max_clearance()
            .max(rules.spec().hole_clearance)
            .max(rules.spec().hole_to_hole)
            .max(edge_clearance)
            .max(own_max)
            .max(0);
        let mut world = World {
            rules,
            edge_clearance,
            layers: (0..n).map(|_| ShapeIndex::new()).collect(),
            meta: vec![Vec::new(); n],
            reach,
            scratch: RefCell::new(Vec::new()),
            locked: Vec::new(),
            clearances: Vec::new(),
        };
        let nets = board
            .pads
            .iter()
            .map(|p| p.net)
            .chain(board.tracks.iter().map(|t| t.net))
            .chain(board.vias.iter().map(|v| v.net))
            .chain(board.zones.iter().map(|z| z.net))
            .flatten()
            .map(|n| n.index() + 1)
            .max()
            .unwrap_or(0);
        world.clearances = (0..nets)
            .flat_map(|i| (0..n).map(move |l| (NetId(i as u32), LayerId(l as u8))))
            .map(|(net, l)| rules.clearance(Some(net), l))
            .collect();
        let all = rules.layers();
        let fixed = |kind, net, own| Meta {
            kind,
            net,
            own,
            owner: Owner::Fixed,
            clearance: 0,
        };
        for t in &board.tracks {
            let shape = match t.mid {
                None => Shape::track(t.start, t.end, t.width)?,
                Some(mid) => Shape::arc_track(t.start, mid, t.end, t.width, board.arc_tolerance)?,
            };
            world.add(
                LayerSet::EMPTY.with(t.layer),
                shape,
                fixed(Kind::Copper, t.net, None),
            );
        }
        for v in &board.vias {
            world.add(
                LayerSet::span(v.from, v.to),
                Shape::circle(v.at, v.diameter)?,
                fixed(Kind::Copper, v.net, None),
            );
            world.add(
                all,
                Shape::circle(v.at, v.drill)?,
                fixed(Kind::Hole, v.net, None),
            );
        }
        for p in &board.pads {
            world.add(
                p.layers,
                p.shape.clone(),
                fixed(Kind::Copper, p.net, p.clearance),
            );
            if let Some(hole) = &p.hole {
                world.add(all, hole.clone(), fixed(Kind::Hole, p.net, p.clearance));
            }
        }
        for z in &board.zones {
            world.add(
                LayerSet::EMPTY.with(z.layer),
                z.shape.clone(),
                fixed(Kind::Copper, z.net, z.clearance),
            );
        }
        for k in &board.keepouts {
            let kind = Kind::Keepout {
                tracks: k.tracks,
                vias: k.vias,
            };
            world.add(k.layers, k.shape.clone(), fixed(kind, None, None));
        }
        for e in outline {
            world.add(all, e.clone(), fixed(Kind::Edge, None, None));
        }
        for m in masks {
            let kind = Kind::Mask {
                margin: m.margin.max(0),
            };
            world.add(
                LayerSet::EMPTY.with(m.layer),
                m.shape.clone(),
                fixed(kind, m.net, None),
            );
        }
        for (shape, layer) in obstacles {
            world.add(
                LayerSet::EMPTY.with(*layer),
                shape.clone(),
                fixed(Kind::Copper, None, None),
            );
        }
        Ok(world)
    }

    pub(crate) fn rules(&self) -> &'r Rules {
        self.rules
    }

    /// [`Rules::clearance`], from the table where it can.
    fn clearance(&self, net: Option<NetId>, layer: LayerId) -> i64 {
        let n = self.layers.len();
        net.and_then(|net| self.clearances.get(net.index() * n + usize::from(layer.0)))
            .copied()
            .unwrap_or_else(|| self.rules.clearance(net, layer))
    }

    /// Makes a net's routes block like fixed copper, or not.
    pub(crate) fn set_locked(&mut self, net: NetId, locked: bool) {
        if net.index() >= self.locked.len() {
            self.locked.resize(net.index() + 1, false);
        }
        self.locked[net.index()] = locked;
    }

    fn add(&mut self, layers: LayerSet, shape: Shape, meta: Meta) -> Placed {
        let mut placed = Placed::default();
        // An arc is indexed by its edges, which are each near far less than the whole.
        let pieces = if meta.owner == Owner::Fixed {
            shape.pieces()
        } else {
            vec![shape]
        };
        for l in layers.iter() {
            let i = usize::from(l.0);
            let meta = Meta {
                clearance: self.clearance(meta.net, l),
                ..meta
            };
            let Some(index) = self.layers.get_mut(i) else {
                continue;
            };
            for piece in &pieces {
                let id = index.insert(piece.clone());
                let slot = id.slot() as usize;
                let meta_l = &mut self.meta[i];
                if slot >= meta_l.len() {
                    meta_l.resize(slot + 1, meta);
                }
                meta_l[slot] = meta;
                placed.entries.push((l.0, id));
            }
        }
        placed
    }

    /// Adds a routed track, and returns what to take out to remove it.
    pub(crate) fn add_track(&mut self, net: NetId, layer: LayerId, shape: Shape) -> Placed {
        let meta = Meta {
            kind: Kind::Copper,
            net: Some(net),
            own: None,
            owner: Owner::Routed(net),
            clearance: 0,
        };
        self.add(LayerSet::EMPTY.with(layer), shape, meta)
    }

    /// Adds a routed through via.
    pub(crate) fn add_via(&mut self, net: NetId, at: Point, diameter: i64, drill: i64) -> Placed {
        let all = self.rules.layers();
        let owner = Owner::Routed(net);
        let mut placed = Placed::default();
        if let (Ok(copper), Ok(hole)) = (Shape::circle(at, diameter), Shape::circle(at, drill)) {
            let copper_meta = Meta {
                kind: Kind::Copper,
                net: Some(net),
                own: None,
                owner,
                clearance: 0,
            };
            let hole_meta = Meta {
                kind: Kind::Hole,
                ..copper_meta
            };
            placed
                .entries
                .extend(self.add(all, copper, copper_meta).entries);
            placed
                .entries
                .extend(self.add(all, hole, hole_meta).entries);
        }
        placed
    }

    /// Takes out what [`add_track`](World::add_track) or [`add_via`](World::add_via) put
    /// in.
    pub(crate) fn remove(&mut self, placed: &Placed) {
        for &(l, id) in &placed.entries {
            self.layers[usize::from(l)].remove(id);
        }
    }

    /// Whether `net`'s track `shape` may be on `layer`, and if only routes are in the way,
    /// which nets they are, added to `routed` if it is given.
    pub(crate) fn track(
        &self,
        net: NetId,
        layer: LayerId,
        shape: &Shape,
        mut routed: Option<&mut Vec<NetId>>,
    ) -> Verdict {
        let l = usize::from(layer.0);
        let Some(index) = self.layers.get(l) else {
            return Verdict::Blocked;
        };
        let mut near = self.scratch.borrow_mut();
        index.query(&shape.bbox().expand(self.reach), &mut near);
        let mut verdict = Verdict::Free;
        let ours = self.clearance(Some(net), layer);
        for &id in near.iter() {
            let meta = self.meta[l][id.slot() as usize];
            let Some(other) = index.get(id) else { continue };
            let hit = match meta.kind {
                Kind::Copper => {
                    !same_net(Some(net), meta.net)
                        && self.copper_hit(shape, other, (net, ours), meta, layer)
                }
                Kind::Hole => {
                    !same_net(Some(net), meta.net) && self.hole_hit(shape, other, None, meta.own)
                }
                Kind::Keepout { tracks, .. } => tracks && shape.touches(other),
                Kind::Edge => self.edge_hit(shape, other),
                Kind::Mask { margin } => {
                    !same_net(Some(net), meta.net) && violates(shape, other, margin + MASK_GUARD)
                }
            };
            if hit && !self.note(meta, &mut verdict, &mut routed) {
                return Verdict::Blocked;
            }
        }
        verdict
    }

    /// Whether `net`'s through via may be at `at`, as [`track`](World::track) says.
    pub(crate) fn via(
        &self,
        net: NetId,
        at: Point,
        diameter: i64,
        drill: i64,
        mut routed: Option<&mut Vec<NetId>>,
    ) -> Verdict {
        let (Ok(copper), Ok(hole)) = (Shape::circle(at, diameter), Shape::circle(at, drill)) else {
            return Verdict::Blocked;
        };
        let mut verdict = Verdict::Free;
        let mut near = self.scratch.borrow_mut();
        for (l, index) in self.layers.iter().enumerate() {
            let layer = LayerId(l as u8);
            let ours = self.clearance(Some(net), layer);
            index.query(&copper.bbox().expand(self.reach), &mut near);
            for &id in near.iter() {
                let meta = self.meta[l][id.slot() as usize];
                let Some(other) = index.get(id) else { continue };
                let other_net = !same_net(Some(net), meta.net);
                let hit = match meta.kind {
                    Kind::Copper => {
                        other_net
                            && (self.copper_hit(&copper, other, (net, ours), meta, layer)
                                || self.hole_hit(other, &hole, None, meta.own))
                    }
                    // Holes go through every layer, so the first one is enough.
                    Kind::Hole if l == 0 => {
                        (other_net && self.hole_hit(&copper, other, meta.own, None))
                            || self.hole_to_hole(&hole, other)
                    }
                    Kind::Hole => false,
                    Kind::Keepout { vias, .. } => vias && copper.touches(other),
                    Kind::Edge => self.edge_hit(&copper, other),
                    Kind::Mask { margin } => {
                        other_net && violates(&copper, other, margin + MASK_GUARD)
                    }
                };
                if hit && !self.note(meta, &mut verdict, &mut routed) {
                    return Verdict::Blocked;
                }
            }
        }
        verdict
    }

    /// Whether the segment from `a` to `b` touches the board's edge.
    pub(crate) fn crosses_edge(&self, a: Point, b: Point) -> bool {
        let Ok(shape) = Shape::track(a, b, 1) else {
            return true;
        };
        let Some(index) = self.layers.first() else {
            return false;
        };
        let mut near = self.scratch.borrow_mut();
        index.query(&shape.bbox(), &mut near);
        near.iter().any(|&id| {
            self.meta[0][id.slot() as usize].kind == Kind::Edge
                && index.get(id).is_some_and(|e| shape.touches(e))
        })
    }

    /// Records a hit on something, and returns whether it can be moved.
    fn note(
        &self,
        meta: Meta,
        verdict: &mut Verdict,
        routed: &mut Option<&mut Vec<NetId>>,
    ) -> bool {
        match meta.owner {
            Owner::Fixed => false,
            Owner::Routed(n) if self.locked.get(n.index()).copied().unwrap_or(false) => false,
            Owner::Routed(n) => {
                *verdict = Verdict::Routed;
                if let Some(out) = routed.as_deref_mut()
                    && !out.contains(&n)
                {
                    out.push(n);
                }
                true
            }
        }
    }

    /// Whether copper of `ours`, a net and its clearance on `layer`, is too close to copper
    /// of another net, as the check decides.
    fn copper_hit(
        &self,
        shape: &Shape,
        other: &Shape,
        ours: (NetId, i64),
        meta: Meta,
        layer: LayerId,
    ) -> bool {
        // What `drc::clearance` comes to, with ours resolved once for the query and theirs
        // when they were added.
        let required = match (meta.own, meta.net) {
            (Some(own), _) => own.max(self.rules.spec().min_clearance),
            (None, Some(theirs)) => match self.rules.pair_clearance(ours.0, theirs) {
                Some(pair) => pair.max(self.rules.spec().min_clearance),
                None => ours.1.max(meta.clearance),
            },
            (None, None) => ours.1.max(meta.clearance),
        };
        debug_assert_eq!(
            required,
            drc::clearance(
                self.rules,
                (Some(ours.0), None),
                (meta.net, meta.own),
                layer
            )
        );
        violates(shape, other, required)
    }

    /// Whether copper is too close to a hole of another net.
    fn hole_hit(
        &self,
        copper: &Shape,
        hole: &Shape,
        hole_own: Option<i64>,
        own: Option<i64>,
    ) -> bool {
        let floor = self.rules.spec().hole_clearance;
        let required = match (hole_own, own) {
            (None, None) => floor,
            (x, y) => x.unwrap_or(0).max(y.unwrap_or(0)).max(floor),
        };
        violates(copper, hole, required)
    }

    fn hole_to_hole(&self, hole: &Shape, other: &Shape) -> bool {
        let required = self.rules.spec().hole_to_hole;
        required > 0 && hole.violates(other, required).unwrap_or(true)
    }

    fn edge_hit(&self, shape: &Shape, edge: &Shape) -> bool {
        violates(shape, edge, self.edge_clearance)
    }
}

/// How far copper keeps from a mask opening beyond its margin, as KiCad's own opening is
/// not exactly ours: a micrometre.
const MASK_GUARD: i64 = 1000;

/// Whether two shapes break a clearance: closer than it, or touching.
fn violates(a: &Shape, b: &Shape, clearance: i64) -> bool {
    if clearance > 0 {
        a.violates(b, clearance).unwrap_or(true)
    } else {
        a.touches(b)
    }
}

/// As in the check: copper with no net is of no net, not even its own.
fn same_net(a: Option<NetId>, b: Option<NetId>) -> bool {
    a.is_some() && a == b
}
