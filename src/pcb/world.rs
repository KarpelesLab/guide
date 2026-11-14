//! Everything routes must keep clear of, indexed by layer, with the exact questions the
//! router asks of it: may a track or a via of a net go here, and if not, whose copper is in
//! the way?
//!
//! The answers follow [`drc::check`] rule for rule, so that what is legal here passes the
//! check: clearances between nets, with those of pairs of nets and of items of their own,
//! hole clearances and hole-to-hole clearances, keepouts with the nets they except, net
//! ties, the openings in the solder mask,
//! and, which the check doesn't have, the clearance to the board's edge. An opening with no
//! net keeps out every net's copper, stricter than the check.

use core::cell::RefCell;

use super::control::Run;
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
    /// A keepout, with whether it keeps out tracks and vias, and which of the world's lists
    /// of nets it excepts, counted from one: zero is none.
    Keepout {
        tracks: bool,
        vias: bool,
        except: u32,
    },
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
    /// Whether it is the copper of a pad with no hole, which vias stay out of where
    /// [`RouteOptions::vias_in_pads`](super::RouteOptions::vias_in_pads) is off.
    smd: bool,
    /// For an item of a net tie, which of the world's lists of nets may touch it, counted
    /// from one: zero is none.
    tie: u32,
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
    /// Where zones of nets are planes, which other nets' tracks pay more to cross.
    planes: Option<Planes>,
    /// Whether a via may touch a pad with no hole.
    vias_in_pads: bool,
    /// The nets that keepouts except, by the number in [`Kind::Keepout`] less one.
    excepts: Vec<Vec<NetId>>,
    /// The nets that may touch the items of net ties, by the number in [`Meta::tie`] less
    /// one.
    ties: Vec<Vec<NetId>>,
    /// Whether any net's tracks are kept off some layer.
    restricted: bool,
    /// By net, the layers its tracks may be on, as a mask, and for nets past the end.
    track_layers: Vec<u64>,
    run_layers: u64,
    /// The run that searches in this world belong to, for its budget and its stop flag.
    run: Option<&'r Run<'r>>,
}

/// Where the planes of nets are, by tile of a grid and layer, and what crossing another
/// net's costs: tracks there cost `1 + cost` times their length.
#[derive(Clone, Debug)]
pub(crate) struct Planes {
    pub(crate) grid: super::detail::Corridor,
    /// By layer and then tile, the net's index plus one, or zero for none.
    pub(crate) nets: Vec<u32>,
    pub(crate) cost: f64,
}

impl<'r> World<'r> {
    /// The fixed items of a board, and its edge.
    pub(crate) fn new(
        board: &Board,
        rules: &'r Rules,
        outline: &[Shape],
        edge_clearance: i64,
        obstacles: &[(Shape, LayerId)],
    ) -> Result<World<'r>, GeomError> {
        let masks = &board.masks;
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
            planes: None,
            vias_in_pads: true,
            excepts: Vec::new(),
            ties: Vec::new(),
            restricted: false,
            track_layers: Vec::new(),
            run_layers: u64::MAX,
            run: None,
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
            smd: false,
            tie: 0,
        };
        // By item of a net tie, its list of nets among the world's.
        let mut tie_of: Vec<(drc::Item, u32)> = Vec::new();
        for (item, nets) in drc::tied_nets(board) {
            world.ties.push(nets);
            tie_of.push((item, world.ties.len() as u32));
        }
        let tie = |item: drc::Item| {
            tie_of
                .binary_search_by_key(&item, |t| t.0)
                .map_or(0, |at| tie_of[at].1)
        };
        for (i, t) in board.tracks.iter().enumerate() {
            let shape = match t.mid {
                None => Shape::track(t.start, t.end, t.width)?,
                Some(mid) => Shape::arc_track(t.start, mid, t.end, t.width, board.arc_tolerance)?,
            };
            world.add(
                LayerSet::EMPTY.with(t.layer),
                shape,
                Meta {
                    tie: tie(drc::Item::Track(i)),
                    ..fixed(Kind::Copper, t.net, None)
                },
            );
        }
        for (i, v) in board.vias.iter().enumerate() {
            let tie = tie(drc::Item::Via(i));
            world.add(
                LayerSet::span(v.from, v.to),
                Shape::circle(v.at, v.diameter)?,
                Meta {
                    tie,
                    ..fixed(Kind::Copper, v.net, None)
                },
            );
            world.add(
                all,
                Shape::circle(v.at, v.drill)?,
                Meta {
                    tie,
                    ..fixed(Kind::Hole, v.net, None)
                },
            );
        }
        for (i, p) in board.pads.iter().enumerate() {
            let tie = tie(drc::Item::Pad(i));
            for (layers, shape) in p.parts() {
                world.add(
                    layers,
                    shape.clone(),
                    Meta {
                        smd: p.hole.is_none(),
                        tie,
                        ..fixed(Kind::Copper, p.net, p.clearance)
                    },
                );
            }
            if let Some(hole) = &p.hole {
                world.add(
                    all,
                    hole.clone(),
                    Meta {
                        tie,
                        ..fixed(Kind::Hole, p.net, p.clearance)
                    },
                );
            }
        }
        for (i, z) in board.zones.iter().enumerate() {
            world.add(
                LayerSet::EMPTY.with(z.layer),
                z.shape.clone(),
                Meta {
                    tie: tie(drc::Item::Zone(i)),
                    ..fixed(Kind::Copper, z.net, z.clearance)
                },
            );
        }
        for k in &board.keepouts {
            let except = if k.except.is_empty() {
                0
            } else {
                world.excepts.push(k.except.clone());
                world.excepts.len() as u32
            };
            let kind = Kind::Keepout {
                tracks: k.tracks,
                vias: k.vias,
                except,
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
            // The opening of a pad of a tie lets the tie's nets in, as the pad does.
            let tie = m.net.map_or(0, |net| {
                let pads = board.pads.iter().enumerate();
                pads.filter(|(_, p)| p.net == Some(net) && p.copper_layers().contains(m.layer))
                    .map(|(i, p)| (tie(drc::Item::Pad(i)), p))
                    .find(|(t, p)| *t != 0 && p.parts().any(|(_, s)| s.touches(&m.shape)))
                    .map_or(0, |(t, _)| t)
            });
            world.add(
                LayerSet::EMPTY.with(m.layer),
                m.shape.clone(),
                Meta {
                    tie,
                    ..fixed(kind, m.net, None)
                },
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

    /// What a run asks of every query: whether vias may be in pads, and the layers that
    /// each net's tracks may be on.
    pub(crate) fn configure(&mut self, design: &super::Design, options: &super::RouteOptions) {
        self.vias_in_pads = options.vias_in_pads;
        let all = self.rules.layers().0;
        self.run_layers = options.layers.map_or(all, |l| l.0);
        self.track_layers = design
            .nets
            .iter()
            .map(|n| self.run_layers & n.layers.map_or(all, |l| l.0))
            .collect();
        self.restricted =
            self.run_layers & all != all || self.track_layers.iter().any(|&mask| mask & all != all);
    }

    /// Whether tracks of `net` may be on a layer.
    #[inline]
    fn on_layer(&self, net: NetId, layer: usize) -> bool {
        let mask = self
            .track_layers
            .get(net.index())
            .unwrap_or(&self.run_layers);
        mask >> layer & 1 != 0
    }

    /// Makes searches in this world part of a run.
    pub(crate) fn set_run(&mut self, run: &'r Run<'r>) {
        self.run = Some(run);
    }

    /// The run that searches in this world belong to, if any.
    pub(crate) fn run(&self) -> Option<&'r Run<'r>> {
        self.run
    }

    /// Sets where the planes are.
    pub(crate) fn set_planes(&mut self, planes: Planes) {
        self.planes = Some(planes);
    }

    /// What a track of `net` on `layer` ending at `at` costs for its length: more inside
    /// another net's plane.
    pub(crate) fn plane_factor(&self, net: NetId, layer: u8, at: Point) -> f64 {
        let Some(p) = &self.planes else {
            return 1.0;
        };
        let Some(tile) = p.grid.tile_of(at) else {
            return 1.0;
        };
        let cells = p.grid.cols * p.grid.rows;
        match p.nets.get(usize::from(layer) * cells + tile) {
            Some(&n) if n != 0 && n != net.0 + 1 => 1.0 + p.cost,
            _ => 1.0,
        }
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
            smd: false,
            tie: 0,
        };
        self.add(LayerSet::EMPTY.with(layer), shape, meta)
    }

    /// Adds a keepout of tracks and vias, as the design's are, for a while.
    pub(crate) fn add_keepout(&mut self, layers: LayerSet, shape: Shape) -> Placed {
        let meta = Meta {
            kind: Kind::Keepout {
                tracks: true,
                vias: true,
                except: 0,
            },
            net: None,
            own: None,
            owner: Owner::Fixed,
            clearance: 0,
            smd: false,
            tie: 0,
        };
        self.add(layers, shape, meta)
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
                smd: false,
                tie: 0,
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
        if self.restricted && !self.on_layer(net, l) {
            return Verdict::Blocked;
        }
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
                        && !self.tied(meta.tie, net)
                        && self.copper_hit(shape, other, (net, ours), meta, layer)
                }
                Kind::Hole => {
                    !same_net(Some(net), meta.net)
                        && !self.tied(meta.tie, net)
                        && self.hole_hit(shape, other, None, meta.own)
                }
                Kind::Keepout { tracks, except, .. } => {
                    tracks && !self.excepted(except, net) && shape.touches(other)
                }
                Kind::Edge => self.edge_hit(shape, other),
                Kind::Mask { margin } => {
                    !same_net(Some(net), meta.net)
                        && !self.tied(meta.tie, net)
                        && violates(shape, other, margin + MASK_GUARD)
                }
            };
            if hit && !self.note(meta, &mut verdict, &mut routed) {
                return Verdict::Blocked;
            }
        }
        verdict
    }

    /// Whether a track that stands for both tracks of a pair may be `shape` on `layer`.
    /// Everything is in its way, the two nets' own copper too, as either track may be the
    /// one beside it: other nets' copper keeps the larger of the two nets' clearances from
    /// it, and copper of the pair's own nets the clearance between the two. As
    /// [`track`](World::track) says, with the nets of routes in the way added to `routed`.
    pub(crate) fn track_pair(
        &self,
        nets: (NetId, NetId),
        layer: LayerId,
        shape: &Shape,
        mut routed: Option<&mut Vec<NetId>>,
    ) -> Verdict {
        let l = usize::from(layer.0);
        let Some(index) = self.layers.get(l) else {
            return Verdict::Blocked;
        };
        if self.restricted && !(self.on_layer(nets.0, l) && self.on_layer(nets.1, l)) {
            return Verdict::Blocked;
        }
        let mut verdict = Verdict::Free;
        let mut near = self.scratch.borrow_mut();
        index.query(&shape.bbox().expand(self.reach), &mut near);
        let (a, b) = (
            (nets.0, self.clearance(Some(nets.0), layer)),
            (nets.1, self.clearance(Some(nets.1), layer)),
        );
        let ours = |m: &Meta| m.net == Some(nets.0) || m.net == Some(nets.1);
        for &id in near.iter() {
            let meta = self.meta[l][id.slot() as usize];
            let Some(other) = index.get(id) else { continue };
            // An item of a tie of both nets is in neither track's way.
            if meta.tie != 0 && self.tied(meta.tie, nets.0) && self.tied(meta.tie, nets.1) {
                continue;
            }
            let hit = match meta.kind {
                Kind::Copper => {
                    let required = if ours(&meta) {
                        self.rules
                            .clearance_between(Some(nets.0), Some(nets.1), layer)
                    } else {
                        self.required(a, meta).max(self.required(b, meta))
                    };
                    violates(shape, other, required)
                }
                Kind::Hole => self.hole_hit(shape, other, None, meta.own),
                // Both tracks are in it, so both nets must be excepted.
                Kind::Keepout { tracks, except, .. } => {
                    tracks
                        && !(self.excepted(except, nets.0) && self.excepted(except, nets.1))
                        && shape.touches(other)
                }
                Kind::Edge => self.edge_hit(shape, other),
                Kind::Mask { margin } => violates(shape, other, margin + MASK_GUARD),
            };
            // The pair's own copper is never moved out of its way.
            if hit && (ours(&meta) || !self.note(meta, &mut verdict, &mut routed)) {
                return Verdict::Blocked;
            }
        }
        verdict
    }

    /// Whether a net's routes are locked, as [`set_locked`](World::set_locked) makes them.
    pub(crate) fn locked(&self, net: NetId) -> bool {
        self.locked.get(net.index()).copied().unwrap_or(false)
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
                let other_net = !same_net(Some(net), meta.net) && !self.tied(meta.tie, net);
                let hit = match meta.kind {
                    Kind::Copper => {
                        (other_net
                            && (self.copper_hit(&copper, other, (net, ours), meta, layer)
                                || self.hole_hit(other, &hole, None, meta.own)))
                            || (!self.vias_in_pads && meta.smd && copper.touches(other))
                    }
                    // Holes go through every layer, so the first one is enough.
                    Kind::Hole if l == 0 => {
                        (other_net && self.hole_hit(&copper, other, meta.own, None))
                            || self.hole_to_hole(&hole, other)
                    }
                    Kind::Hole => false,
                    Kind::Keepout { vias, except, .. } => {
                        vias && !self.excepted(except, net) && copper.touches(other)
                    }
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

    /// Whether a net tie's list of nets, by its number, holds `net`.
    #[inline]
    fn tied(&self, tie: u32, net: NetId) -> bool {
        tie != 0 && self.ties[tie as usize - 1].contains(&net)
    }

    /// Whether a keepout's list of excepted nets, by its number, holds `net`.
    #[inline]
    fn excepted(&self, except: u32, net: NetId) -> bool {
        except != 0 && self.excepts[except as usize - 1].contains(&net)
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
        let required = self.required(ours, meta);
        debug_assert_eq!(
            required,
            drc::clearance(
                self.rules,
                (Some(ours.0), None),
                (meta.net, meta.own),
                layer
            )
        );
        // Both in an area with a clearance of its own, that one applies.
        let required = self
            .rules
            .area_clearance(layer, shape, other)
            .unwrap_or(required);
        violates(shape, other, required)
    }

    /// The clearance between copper of `ours`, a net and its clearance on the layer, and an
    /// entry: what `drc::clearance` comes to, with ours resolved once for the query and
    /// theirs when they were added.
    fn required(&self, ours: (NetId, i64), meta: Meta) -> i64 {
        match (meta.own, meta.net) {
            (Some(own), _) => own.max(self.rules.spec().min_clearance),
            (None, Some(theirs)) => match self.rules.pair_clearance(ours.0, theirs) {
                Some(pair) => pair.max(self.rules.spec().min_clearance),
                None => ours.1.max(meta.clearance),
            },
            (None, None) => ours.1.max(meta.clearance),
        }
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
