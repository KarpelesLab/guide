//! What a run reports beside its copper: which item of a design is at fault in an error,
//! what each track and via of the result is, and the connections left open, each with the
//! pads on either side and the best-known cause.
//!
//! Open connections are counted on the result, not from the router's bookkeeping: a net's
//! pads, through its copper and the zones' fills, are in pieces, and the pieces less one are
//! the connections open, as in a ratsnest. A cause is what the searches said of the net, and
//! for a net that found no path, a look around the pad left out: a flood from it over the
//! finest lattice the run used, a thousand nodes at most. If the flood ends, the pad is shut
//! in, and what stopped it is named.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::detail::{DIRS, Failure};
use super::planes;
use super::router::{pad_center, terminals};
use super::world::{Verdict, World};
use super::{Design, Stopped};
use crate::geom::drc::{Board, Item, Track, Via, tied_nets};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::NetId;

/// An item of a [`Design`], by its position in the list that holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DesignItem {
    /// A pad, in [`Board::pads`].
    Pad(usize),
    /// A fixed track, in [`Board::tracks`].
    Track(usize),
    /// A fixed via, in [`Board::vias`].
    Via(usize),
    /// A filled zone, in [`Board::zones`].
    Zone(usize),
    /// A keepout, in [`Board::keepouts`].
    Keepout(usize),
    /// An opening in the solder mask, in [`Board::masks`].
    Mask(usize),
    /// An edge of the board, in [`Design::outline`].
    Outline(usize),
    /// Copper with no net, in [`Design::obstacles`].
    Obstacle(usize),
}

impl fmt::Display for DesignItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, i) = match *self {
            DesignItem::Pad(i) => ("pad", i),
            DesignItem::Track(i) => ("track", i),
            DesignItem::Via(i) => ("via", i),
            DesignItem::Zone(i) => ("zone", i),
            DesignItem::Keepout(i) => ("keepout", i),
            DesignItem::Mask(i) => ("mask opening", i),
            DesignItem::Outline(i) => ("outline edge", i),
            DesignItem::Obstacle(i) => ("obstacle", i),
        };
        write!(f, "{what} {i}")
    }
}

/// What a track or a via of a result is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum CopperKind {
    /// A route between pads.
    Plain,
    /// An escape from a fine-pitch footprint, placed before routing.
    Escape,
    /// A differential pair's, routed side by side, with its fan-out.
    Pair,
    /// What joins a net to its zone, or the pieces of a net with zones to each other:
    /// the routes of a net whose terminals a zone's fill joins, and the stubs added after
    /// the zones are filled.
    Stub,
    /// A track that a meander replaced, for a length.
    Meander,
    /// Copper of an earlier result that [`eco::reroute`](super::eco::reroute) kept.
    Kept,
}

/// One end of an [`OpenConnection`]: a pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Endpoint {
    /// The pad, by position in [`Board::pads`].
    pub pad: Option<usize>,
    /// A point inside it, where routes end.
    pub at: Point,
    /// The layers it has copper on.
    pub layers: LayerSet,
}

/// What stands in a route's way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Blocker {
    /// An item of the design.
    Item(DesignItem),
    /// Copper of another net in this result.
    Net(NetId),
}

/// Why a connection is open, as far as the run knows.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cause {
    /// The net has no via, and the pads on either side share no layer.
    NoLayer,
    /// The pad at [`OpenConnection::to`] is shut in: every way out of it, on the finest
    /// lattice the run used, ends within a thousand nodes at one of these, eight at most:
    /// other nets' routes first, then what stops its tracks, then what stops its vias.
    /// Empty if only the bounds of the board stopped it.
    Enclosed {
        /// What shuts it in.
        by: Vec<Blocker>,
    },
    /// The searches for the net ended with nowhere left to go, and the pad is not shut in
    /// nearby.
    NoPath,
    /// A search for the net ran out of nodes, [`RouteOptions::max_expansions`](super::RouteOptions::max_expansions).
    OutOfBudget,
    /// The run was stopped before the net was routed, or routed again after a rip-up.
    Stopped,
    /// The router took the net as routed or had no say on it: a net with zones whose fills
    /// leave it in pieces, for one.
    Unknown,
}

/// A connection the result leaves open: two pieces of a net's copper to join, with a pad of
/// each, the two nearest.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenConnection {
    /// The net.
    pub net: NetId,
    /// A pad of the piece it would join to: the net's largest, or one joined to that by
    /// the connections before this one in the list.
    pub from: Endpoint,
    /// A pad of the piece left out.
    pub to: Endpoint,
    /// Why it is open.
    pub cause: Cause,
}

/// A connection left open, before its cause is known.
pub(crate) struct Open {
    net: NetId,
    from: usize,
    to: usize,
    /// The pads of the piece left out.
    piece: Vec<usize>,
}

/// A pad as an end.
fn endpoint(design: &Design, pad: usize) -> Endpoint {
    let p = &design.board.pads[pad];
    let b = p.shape.bbox();
    let at = pad_center(&p.shape).unwrap_or(Point::new(
        b.min().x + (b.max().x - b.min().x) / 2,
        b.min().y + (b.max().y - b.min().y) / 2,
    ));
    Endpoint {
        pad: Some(pad),
        at,
        layers: LayerSet(p.copper_layers().0 & design.rules.layers().0),
    }
}

/// What a board leaves of a design's nets.
pub(crate) struct Survey {
    /// The nets that need connecting.
    pub(crate) nets: usize,
    /// Of those, the nets in one piece.
    pub(crate) done: usize,
    /// The connections needed.
    pub(crate) needed: usize,
    /// Of those, the connections made.
    pub(crate) made: usize,
    /// The nets left in pieces, in order.
    pub(crate) broken: Vec<NetId>,
    /// The connections left open.
    pub(crate) open: Vec<Open>,
}

/// The connections a host counts, on `board`, the design's with a result's tracks, vias
/// and fills: a net needs connecting if its pads are in pieces that the design's own copper
/// doesn't join, and a connection is made for each piece fewer that the board leaves it in.
/// The pieces left are joined into a tree from the largest, by the nearest pads, which are
/// the connections open.
pub(crate) fn survey(design: &Design, board: &Board) -> Survey {
    let (mut nets, mut done, mut needed, mut made) = (0, 0, 0, 0);
    let mut broken = Vec::new();
    let mut open = Vec::new();
    let stack = design.rules.layers().0;
    for (i, spec) in design.nets.iter().enumerate() {
        let net = NetId(i as u32);
        if !spec.route {
            continue;
        }
        let groups = terminals(design, net)
            .into_iter()
            .filter(|(pads, _)| {
                pads.iter()
                    .any(|&p| design.board.pads[p].copper_layers().0 & stack != 0)
            })
            .count();
        if groups < 2 {
            continue;
        }
        let pieces = planes::pieces(board, design, net, &[]).pads;
        let left = pieces.len().clamp(1, groups);
        nets += 1;
        needed += groups - 1;
        made += groups - left;
        if left == 1 {
            done += 1;
        } else {
            broken.push(net);
        }
        if pieces.len() < 2 {
            continue;
        }
        let at: BTreeMap<usize, Point> = pieces
            .iter()
            .flatten()
            .map(|&p| (p, endpoint(design, p).at))
            .collect();
        let far = |a: usize, b: usize| {
            let (dx, dy) = ((at[&a].x - at[&b].x) as i128, (at[&a].y - at[&b].y) as i128);
            dx * dx + dy * dy
        };
        let first = (0..pieces.len())
            .max_by_key(|&k| (pieces[k].len(), core::cmp::Reverse(k)))
            .unwrap_or(0);
        let mut inside = vec![false; pieces.len()];
        inside[first] = true;
        // By piece outside the tree, its nearest pad to the tree: distance, its pad, the
        // tree's.
        let mut best: Vec<Option<(i128, usize, usize)>> = vec![None; pieces.len()];
        let update =
            |best: &mut Vec<Option<(i128, usize, usize)>>, joined: usize, inside: &[bool]| {
                for (k, piece) in pieces.iter().enumerate().filter(|(k, _)| !inside[*k]) {
                    for &to in piece {
                        for &from in &pieces[joined] {
                            let d = (far(from, to), to, from);
                            if best[k].is_none_or(|b| d < b) {
                                best[k] = Some(d);
                            }
                        }
                    }
                }
            };
        update(&mut best, first, &inside);
        let mut found = Vec::new();
        while let Some((k, (_, to, from))) = (0..pieces.len())
            .filter(|&k| !inside[k])
            .filter_map(|k| best[k].map(|b| (k, b)))
            .min_by_key(|&(k, b)| (b, k))
        {
            inside[k] = true;
            found.push(Open {
                net,
                from,
                to,
                piece: pieces[k].clone(),
            });
            update(&mut best, k, &inside);
        }
        found.sort_by_key(|o| o.to);
        open.extend(found);
    }
    Survey {
        nets,
        done,
        needed,
        made,
        broken,
        open,
    }
}

/// What the run knows of why nets are open.
pub(crate) struct Known<'a> {
    /// By net, how its last search that failed ended.
    pub(crate) failed: &'a BTreeMap<NetId, Failure>,
    /// Why the run stopped, if it did.
    pub(crate) stopped: Option<Stopped>,
    /// The finest lattice the run searched on.
    pub(crate) pitch: i64,
    /// Where routes may go.
    pub(crate) bounds: Rect,
}

/// The open connections with their causes. `world` holds the design's items and the
/// result's copper as routes, with no net locked.
pub(crate) fn explain(
    design: &Design,
    world: &World<'_>,
    open: Vec<Open>,
    known: &Known<'_>,
) -> Vec<OpenConnection> {
    let mut items: Option<Items> = None;
    let stack = design.rules.layers().0;
    open.into_iter()
        .map(|o| {
            let spec = design.nets.get(o.net.index());
            let via = spec
                .and_then(|s| s.via)
                .filter(|_| design.rules.layer_count() > 1);
            let layers = |pads: &mut dyn Iterator<Item = usize>| {
                pads.fold(0u64, |m, p| {
                    m | (design.board.pads[p].copper_layers().0 & stack)
                })
            };
            let piece = layers(&mut o.piece.iter().copied());
            let rest = layers(
                &mut (0..design.board.pads.len())
                    .filter(|p| design.board.pads[*p].net == Some(o.net) && !o.piece.contains(p)),
            );
            let timed = matches!(known.stopped, Some(Stopped::Deadline | Stopped::Cancelled));
            let cause = if via.is_none() && piece & rest == 0 {
                Cause::NoLayer
            } else {
                match known.failed.get(&o.net) {
                    Some(Failure::Budget) => Cause::OutOfBudget,
                    Some(Failure::NoPath) if timed => Cause::NoPath,
                    None if known.stopped.is_some() => Cause::Stopped,
                    failed => {
                        let items = items.get_or_insert_with(|| Items::new(design));
                        match enclosed(design, world, items, o.net, o.to, known) {
                            Some(by) => Cause::Enclosed { by },
                            None if failed.is_some() => Cause::NoPath,
                            None => Cause::Unknown,
                        }
                    }
                }
            };
            OpenConnection {
                net: o.net,
                from: endpoint(design, o.from),
                to: endpoint(design, o.to),
                cause,
            }
        })
        .collect()
}

/// What an item is to a route, for naming what blocks one.
#[derive(Clone, Copy)]
enum Kind {
    Copper {
        own: Option<i64>,
    },
    Hole {
        own: Option<i64>,
    },
    /// With its position in the board's keepouts, for the nets it excepts.
    Keepout {
        tracks: bool,
        vias: bool,
        at: usize,
    },
    Edge,
    Mask {
        margin: i64,
    },
}

/// The design's items that can block a route, to find which of them does.
struct Items {
    /// By item of a net tie, the nets that may touch it.
    ties: BTreeMap<Item, Vec<NetId>>,
    index: SpatialIndex,
    list: Vec<(Blocker, Kind, Shape, LayerSet, Option<NetId>)>,
    reach: i64,
}

impl Items {
    fn new(design: &Design) -> Items {
        let b = &design.board;
        let all = design.rules.layers();
        let on = |l: LayerId| LayerSet::EMPTY.with(l);
        let mut list = Vec::new();
        let copper = Kind::Copper { own: None };
        for (i, p) in b.pads.iter().enumerate() {
            let it = Blocker::Item(DesignItem::Pad(i));
            let own = p.clearance;
            for (layers, shape) in p.parts() {
                list.push((it, Kind::Copper { own }, shape.clone(), layers, p.net));
            }
            if let Some(h) = &p.hole {
                list.push((it, Kind::Hole { own }, h.clone(), all, p.net));
            }
        }
        let mut track = |it: Blocker, t: &Track| {
            let shape = match t.mid {
                None => Shape::track(t.start, t.end, t.width),
                Some(m) => Shape::arc_track(t.start, m, t.end, t.width, b.arc_tolerance),
            };
            if let Ok(s) = shape {
                list.push((it, copper, s, on(t.layer), t.net));
            }
        };
        for (i, t) in b.tracks.iter().enumerate() {
            track(Blocker::Item(DesignItem::Track(i)), t);
        }
        let mut via = |it: Blocker, v: &Via| {
            if let (Ok(c), Ok(h)) = (
                Shape::circle(v.at, v.diameter),
                Shape::circle(v.at, v.drill),
            ) {
                list.push((it, copper, c, LayerSet::span(v.from, v.to), v.net));
                list.push((it, Kind::Hole { own: None }, h, all, v.net));
            }
        };
        for (i, v) in b.vias.iter().enumerate() {
            via(Blocker::Item(DesignItem::Via(i)), v);
        }
        for (i, z) in b.zones.iter().enumerate() {
            let it = Blocker::Item(DesignItem::Zone(i));
            let kind = Kind::Copper { own: z.clearance };
            list.push((it, kind, z.shape.clone(), on(z.layer), z.net));
        }
        for (i, k) in b.keepouts.iter().enumerate() {
            let kind = Kind::Keepout {
                tracks: k.tracks,
                vias: k.vias,
                at: i,
            };
            let it = Blocker::Item(DesignItem::Keepout(i));
            list.push((it, kind, k.shape.clone(), k.layers, None));
        }
        for (i, m) in b.masks.iter().enumerate() {
            let kind = Kind::Mask {
                margin: m.margin.max(0) + 1000,
            };
            let it = Blocker::Item(DesignItem::Mask(i));
            list.push((it, kind, m.shape.clone(), on(m.layer), m.net));
        }
        for (i, e) in design.outline.iter().enumerate() {
            let it = Blocker::Item(DesignItem::Outline(i));
            list.push((it, Kind::Edge, e.clone(), all, None));
        }
        for (i, (s, l)) in design.obstacles.iter().enumerate() {
            let it = Blocker::Item(DesignItem::Obstacle(i));
            list.push((it, copper, s.clone(), on(*l), None));
        }
        let spec = design.rules.spec();
        let own = list
            .iter()
            .filter_map(|(_, k, ..)| match k {
                Kind::Copper { own } | Kind::Hole { own } => *own,
                Kind::Mask { margin } => Some(*margin),
                _ => None,
            })
            .fold(0, i64::max);
        Items {
            ties: tied_nets(b),
            index: SpatialIndex::bulk_load(list.iter().map(|(_, _, s, ..)| s.bbox())),
            reach: design
                .rules
                .max_clearance()
                .max(spec.hole_clearance)
                .max(spec.hole_to_hole)
                .max(design.edge_clearance)
                .max(own)
                .max(0),
            list,
        }
    }

    /// Whether a net tie lets `net` touch an item.
    fn tied(&self, it: Blocker, net: NetId) -> bool {
        let item = match it {
            Blocker::Item(DesignItem::Pad(i)) => Item::Pad(i),
            Blocker::Item(DesignItem::Track(i)) => Item::Track(i),
            Blocker::Item(DesignItem::Via(i)) => Item::Via(i),
            Blocker::Item(DesignItem::Zone(i)) => Item::Zone(i),
            _ => return false,
        };
        self.ties.get(&item).is_some_and(|nets| nets.contains(&net))
    }

    /// What is in the way of `net`'s copper `shape` on `layers`, a track's or a via's with
    /// its `hole`, added to `out`.
    fn blocking(
        &self,
        design: &Design,
        net: NetId,
        shape: &Shape,
        hole: Option<&Shape>,
        layers: LayerSet,
        out: &mut BTreeSet<Blocker>,
    ) {
        let rules = &design.rules;
        let spec = rules.spec();
        let close = |a: &Shape, b: &Shape, c: i64| {
            if c > 0 {
                a.violates(b, c).unwrap_or(true)
            } else {
                a.touches(b)
            }
        };
        let mut near = Vec::new();
        self.index
            .query(&shape.bbox().expand(self.reach), &mut near);
        for id in near {
            let (it, kind, other, on, other_net) = &self.list[id.slot() as usize];
            if on.0 & layers.0 == 0 || self.tied(*it, net) {
                continue;
            }
            let foreign = *other_net != Some(net);
            let hit = match *kind {
                Kind::Copper { own } => {
                    let required = own.map_or_else(
                        || {
                            LayerSet(on.0 & layers.0)
                                .iter()
                                .map(|l| rules.clearance_between(Some(net), *other_net, l))
                                .fold(0, i64::max)
                        },
                        |c| c.max(spec.min_clearance),
                    );
                    foreign
                        && (close(shape, other, required)
                            || hole.is_some_and(|h| close(other, h, spec.hole_clearance)))
                }
                Kind::Hole { own } => {
                    (foreign && close(shape, other, own.unwrap_or(0).max(spec.hole_clearance)))
                        || hole.is_some_and(|h| {
                            spec.hole_to_hole > 0 && close(h, other, spec.hole_to_hole)
                        })
                }
                Kind::Keepout { tracks, vias, at } => {
                    (if hole.is_some() { vias } else { tracks })
                        && !design.board.keepouts[at].except.contains(&net)
                        && shape.touches(other)
                }
                Kind::Edge => close(shape, other, design.edge_clearance),
                Kind::Mask { margin } => foreign && close(shape, other, margin),
            };
            if hit {
                out.insert(*it);
            }
        }
    }
}

/// How many nodes a flood from a pad goes through to find it shut in.
const FLOOD: usize = 1000;

/// How many blockers a cause names.
const BLOCKERS: usize = 8;

/// Whether `pad` of `net` is shut in, and by what: a flood from it over the lattice, by
/// tracks and vias as the world allows them, that ends within [`FLOOD`] nodes.
fn enclosed(
    design: &Design,
    world: &World<'_>,
    items: &Items,
    net: NetId,
    pad: usize,
    known: &Known<'_>,
) -> Option<Vec<Blocker>> {
    let rules = &design.rules;
    let spec = design.nets.get(net.index())?;
    let end = endpoint(design, pad);
    let layers = rules.layer_count();
    let via = spec.via.filter(|_| layers > 1);
    let pitch = known.pitch.max(1);
    let b = known.bounds;
    let mut seen: BTreeSet<(i64, i64, u8)> = BTreeSet::new();
    let mut queue: VecDeque<(Point, u8)> = VecDeque::new();
    for l in end.layers.iter() {
        seen.insert((end.at.x, end.at.y, l.0));
        queue.push_back((end.at, l.0));
    }
    // What stops tracks, what stops vias, and the nets whose routes stop either.
    let mut by: BTreeSet<Blocker> = BTreeSet::new();
    let mut by_via: BTreeSet<Blocker> = BTreeSet::new();
    let mut nets: Vec<NetId> = Vec::new();
    while let Some((p, l)) = queue.pop_front() {
        if seen.len() > FLOOD {
            return None;
        }
        let layer = LayerId(l);
        let width = spec.track_width.max(rules.track_width(Some(net), layer));
        for (dx, dy) in DIRS {
            let to = Point::new(p.x + dx * pitch, p.y + dy * pitch);
            if to.x < b.min().x || to.y < b.min().y || to.x > b.max().x || to.y > b.max().y {
                continue;
            }
            if seen.contains(&(to.x, to.y, l)) {
                continue;
            }
            let Ok(shape) = Shape::track(p, to, width) else {
                continue;
            };
            match world.track(net, layer, &shape, Some(&mut nets)) {
                Verdict::Free => {
                    seen.insert((to.x, to.y, l));
                    queue.push_back((to, l));
                }
                Verdict::Routed => {}
                Verdict::Blocked => {
                    items.blocking(
                        design,
                        net,
                        &shape,
                        None,
                        LayerSet::EMPTY.with(layer),
                        &mut by,
                    );
                }
            }
        }
        if let Some(v) = via
            && (0..layers as u8).any(|k| !seen.contains(&(p.x, p.y, k)))
        {
            match world.via(net, p, v.diameter, v.drill, Some(&mut nets)) {
                Verdict::Free => {
                    for k in 0..layers as u8 {
                        if seen.insert((p.x, p.y, k)) {
                            queue.push_back((p, k));
                        }
                    }
                }
                Verdict::Routed => {}
                Verdict::Blocked => {
                    if let (Ok(c), Ok(h)) =
                        (Shape::circle(p, v.diameter), Shape::circle(p, v.drill))
                    {
                        items.blocking(design, net, &c, Some(&h), rules.layers(), &mut by_via);
                    }
                }
            }
        }
    }
    // Routes first, which a host can move, then what stops tracks, then what stops vias.
    nets.sort_unstable();
    let mut out: Vec<Blocker> = nets.into_iter().map(Blocker::Net).collect();
    for it in by.into_iter().chain(by_via) {
        if !out.contains(&it) {
            out.push(it);
        }
    }
    out.truncate(BLOCKERS);
    Some(out)
}
