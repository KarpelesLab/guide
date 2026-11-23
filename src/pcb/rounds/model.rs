//! The router's own view of the board: nets with their rule profiles, obstacles with the
//! clearance they demand, drilled holes, the board outline and the connectable copper
//! (terminals) grouped in islands, made from a [`Design`].

use std::collections::BTreeMap;

use crate::geom::drc::Board;
use crate::geom::{LayerId, LayerSet, Shape as GShape, SpatialIndex};
use crate::id::NetId;
use crate::pcb::{Design, Fill as GFill};

use super::geo::{BoxF, Nm, P, Poly, Shape, point_seg_d2, seg_seg_dist};
use super::index::TOL;
use super::scoped::{Kind, Scoped, St, Who};

/// Routing rules of a net: track width, clearance and via size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProfileKey {
    pub width: Nm,
    pub clear: Nm,
    pub via_drill: Nm,
    pub via_dia: Nm,
}

/// A rule profile with its values as floats (half widths and radii).
#[derive(Clone, Debug)]
pub(crate) struct Profile {
    pub key: ProfileKey,
    /// Half track width.
    pub hw: f64,
    /// Clearance.
    pub c: f64,
    /// Via pad radius.
    pub rv: f64,
    /// Via drill radius.
    pub dr: f64,
    /// How far the copper's own centerlines may lie from the routed centerline (0 here), for
    /// scoped rules.
    pub inner: f64,
}

/// How much room an obstacle demands.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Clear {
    /// Copper of a net with this clearance: the larger of both nets' clearances applies.
    Net(f64),
    /// A fixed distance (board edge, keep-outs).
    Fixed(f64),
}

impl Clear {
    pub fn with(self, c: f64) -> f64 {
        match self {
            Clear::Net(x) => x.max(c),
            Clear::Fixed(x) => x,
        }
    }
}

/// What an obstacle is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObKind {
    /// A pad.
    Pad,
    /// An existing track or via.
    Copper,
    /// A board edge or cutout.
    Edge,
    /// A keep-out area.
    Keepout,
    /// A non-plated hole.
    Npth,
}

/// Something routing must keep away from.
#[derive(Clone, Debug)]
pub(crate) struct Obstacle {
    pub shape: Shape,
    pub bbox: BoxF,
    /// Copper layers (bit per stackup layer) where tracks must keep away.
    pub tracks: u64,
    /// Whether vias must keep away.
    pub vias: bool,
    /// Net of the copper (`None`: blocks every net).
    pub net: Option<u32>,
    /// Nets it does not exist for (a keep-out's excepted nets; sorted). Empty for anything else.
    pub except: Vec<u32>,
    pub clear: Clear,
    pub kind: ObKind,
    /// Part owning it (pads).
    pub owner: Option<String>,
    /// The copper item it is (index into the items the board was built from), whose clearance
    /// the scoped rules resolve exactly (`None`: `clear` decides).
    pub item: Option<u32>,
}

impl Obstacle {
    /// Whether tracks and vias of `net` ignore it (a keep-out excepting the net).
    #[inline]
    pub fn spares(&self, net: u32) -> bool {
        !self.except.is_empty() && self.except.binary_search(&net).is_ok()
    }
}

/// A drilled hole (vias keep `hole_to_hole` from it).
#[derive(Clone, Debug)]
pub(crate) struct Hole {
    pub at: P,
    pub r: f64,
}

/// Where a stub to a terminal ends.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Anchor {
    /// Pad or via center.
    Center(P),
    /// Along a track's centerline.
    Segment(P, P),
}

/// What a copper item of the board is, by position in the board's lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum What {
    /// A shape of a pad: the pad, and whether it is its first shape.
    Pad(usize, bool),
    /// A track.
    Track(usize),
    /// A via.
    Via(usize),
    /// A copper drawing (a zone of the board model).
    Drawing(usize),
}

/// A copper item of the board with a shape of the design's own.
#[derive(Clone, Debug)]
pub(crate) struct Copper {
    pub shape: GShape,
    pub layers: LayerSet,
    pub net: Option<NetId>,
    /// A clearance of its own.
    pub own: Option<i64>,
    pub what: What,
}

/// The copper items of `board` (the design's, or the design's with more tracks and vias) on
/// the layers `stack`: each shape of each pad, the tracks, the vias and the copper drawings.
pub(crate) fn copper(board: &Board, stack: LayerSet) -> Vec<Copper> {
    let mut out = Vec::new();
    for (i, pad) in board.pads.iter().enumerate() {
        for (k, (layers, shape)) in pad.parts().enumerate() {
            let layers = LayerSet(layers.0 & stack.0);
            if layers.0 != 0 {
                out.push(Copper {
                    shape: shape.clone(),
                    layers,
                    net: pad.net,
                    own: pad.clearance,
                    what: What::Pad(i, k == 0),
                });
            }
        }
    }
    for (i, t) in board.tracks.iter().enumerate() {
        let shape = match t.mid {
            None => GShape::track(t.start, t.end, t.width),
            Some(m) => GShape::arc_track(t.start, m, t.end, t.width, board.arc_tolerance),
        };
        if let Ok(shape) = shape {
            let layers = LayerSet(LayerSet::EMPTY.with(t.layer).0 & stack.0);
            out.push(Copper {
                shape,
                layers,
                net: t.net,
                own: None,
                what: What::Track(i),
            });
        }
    }
    for (i, v) in board.vias.iter().enumerate() {
        if let Ok(shape) = GShape::circle(v.at, v.diameter) {
            let layers = LayerSet(LayerSet::span(v.from, v.to).0 & stack.0);
            out.push(Copper {
                shape,
                layers,
                net: v.net,
                own: None,
                what: What::Via(i),
            });
        }
    }
    for (i, z) in board.zones.iter().enumerate() {
        let layers = LayerSet(LayerSet::EMPTY.with(z.layer).0 & stack.0);
        out.push(Copper {
            shape: z.shape.clone(),
            layers,
            net: z.net,
            own: z.clearance,
            what: What::Drawing(i),
        });
    }
    out
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Copper islands: the island of every item of `items` and then of every fill of `fills`
/// (with its net), the lowest position among the items and fills of the same net that touch
/// on a layer both are on; the shapes of one pad are one island through its hole. Copper with
/// no net is on its own.
pub(crate) fn islands(items: &[Copper], fills: &[(&GFill, NetId)]) -> Vec<usize> {
    let n = items.len() + fills.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut by_net: BTreeMap<NetId, Vec<usize>> = BTreeMap::new();
    let mut pad_first: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, it) in items.iter().enumerate() {
        if let Some(net) = it.net {
            by_net.entry(net).or_default().push(i);
        }
        if let What::Pad(p, _) = it.what {
            let f = *pad_first.entry(p).or_insert(i);
            parent[i] = f;
        }
    }
    for (k, (_, net)) in fills.iter().enumerate() {
        by_net.entry(*net).or_default().push(items.len() + k);
    }
    let shape = |i: usize| -> (&GShape, u64) {
        if i < items.len() {
            (&items[i].shape, items[i].layers.0)
        } else {
            let f = fills[i - items.len()].0;
            (&f.shape, LayerSet::EMPTY.with(f.layer).0)
        }
    };
    let mut near = Vec::new();
    for list in by_net.values() {
        if list.len() < 2 {
            continue;
        }
        let index = SpatialIndex::bulk_load(list.iter().map(|&i| shape(i).0.bbox()));
        for (a, &i) in list.iter().enumerate() {
            let (si, li) = shape(i);
            index.query(&si.bbox(), &mut near);
            for id in &near {
                let b = id.slot() as usize;
                if b <= a {
                    continue;
                }
                let j = list[b];
                let (sj, lj) = shape(j);
                if li & lj == 0 {
                    continue;
                }
                let (x, y) = (find(&mut parent, i), find(&mut parent, j));
                if x != y && si.touches(sj) {
                    parent[x.max(y)] = x.min(y);
                }
            }
        }
    }
    (0..n).map(|i| find(&mut parent, i)).collect()
}

/// A connectable piece of copper of a net: pad, via or track.
#[derive(Clone, Debug)]
pub(crate) struct Terminal {
    pub net: u32,
    pub shape: Shape,
    /// Copper layers (bit per stackup layer).
    pub layers: u64,
    pub anchor: Anchor,
    /// `P1.3` (part and pad), `via#4`, `track#7`.
    pub label: String,
    /// Copper island (index of its lowest item).
    pub island: usize,
    /// Whether this is a pad (ratsnest endpoint).
    pub pad: bool,
    /// Representative point.
    pub at: P,
    /// The item it is.
    pub what: What,
    /// Its position among the items the board was built from.
    pub item: usize,
}

/// A zone fill island of a net on one layer: copper a connection of the net may end on
/// (`pours`). Not an obstacle: fills are refilled around new copper.
#[derive(Clone, Debug)]
pub(crate) struct Fill {
    pub net: u32,
    /// Stackup layer index.
    pub layer: usize,
    /// Copper island (as [`Terminal::island`]).
    pub island: usize,
    /// Outer rings and holes of the fill's polygons (inside by the even-odd rule).
    pub rings: Vec<Vec<P>>,
    pub bbox: BoxF,
}

/// The board's own values that the router reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BoardRules {
    /// The default clearance (nm).
    pub clearance: f64,
    pub copper_to_edge: Nm,
    pub min_track_width: Nm,
    pub min_drill: Nm,
    pub min_annular_ring: Nm,
}

/// The router's board.
pub(crate) struct RouterBoard {
    pub layers: usize,
    pub net_profile: Vec<usize>,
    pub profiles: Vec<Profile>,
    pub obstacles: Vec<Obstacle>,
    pub holes: Vec<Hole>,
    pub outer: Option<Poly>,
    pub cutouts: Vec<Poly>,
    pub bbox: BoxF,
    pub h2h: f64,
    pub rules: BoardRules,
    pub terminals: Vec<Terminal>,
    /// Zone fills of nets, per island and layer.
    pub fills: Vec<Fill>,
    /// Rule areas, clearances of items' own and the hole clearance, for new copper.
    pub scoped: Scoped,
}

/// The fills `fills` (each with its net) as the router has them, with their islands `isl`
/// (the islands of the items and fills together, the fills after `first` items).
pub(crate) fn fills_of(fills: &[(&GFill, NetId)], isl: &[usize], first: usize) -> Vec<Fill> {
    let mut out = Vec::new();
    for (k, (f, net)) in fills.iter().enumerate() {
        let Some(rings) = f.shape.rings() else {
            continue;
        };
        let rings: Vec<Vec<P>> = rings
            .iter()
            .map(|r| r.iter().map(|q| P::of(*q)).collect())
            .collect();
        let mut bbox = BoxF::EMPTY;
        for r in &rings {
            r.iter().for_each(|&q| bbox.add(q));
        }
        out.push(Fill {
            net: net.0,
            layer: usize::from(f.layer.0),
            island: isl[first + k],
            rings,
            bbox,
        });
    }
    out
}

/// The rings of an outline given as its sides: each closed chain of sides, in order.
fn outline_rings(sides: &[GShape]) -> Vec<Vec<P>> {
    let ends: Vec<(crate::geom::Point, crate::geom::Point)> = sides
        .iter()
        .filter_map(|s| {
            let (c, _, _) = s.core_corners();
            (c.len() == 2).then(|| (c[0], c[1]))
        })
        .collect();
    let mut at: BTreeMap<crate::geom::Point, Vec<usize>> = BTreeMap::new();
    for (k, &(a, b)) in ends.iter().enumerate() {
        at.entry(a).or_default().push(k);
        at.entry(b).or_default().push(k);
    }
    let mut used = vec![false; ends.len()];
    let mut rings = Vec::new();
    for k0 in 0..ends.len() {
        if used[k0] {
            continue;
        }
        used[k0] = true;
        let (start, mut here) = ends[k0];
        let mut ring = vec![P::of(start)];
        while here != start {
            ring.push(P::of(here));
            let Some(&k) = at.get(&here).and_then(|v| v.iter().find(|&&k| !used[k])) else {
                break;
            };
            used[k] = true;
            here = if ends[k].0 == here {
                ends[k].1
            } else {
                ends[k].0
            };
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    rings
}

impl RouterBoard {
    /// Builds the router's view of `design` with `board` as its copper (the design's own, or
    /// that with more tracks and vias): `items` are that board's copper items
    /// ([`copper`]), `fills` the zone fills with their nets and `isl` the islands of both
    /// ([`islands`]); `wall` says which of the fills are copper to the other nets.
    pub fn build(
        design: &Design,
        board: &Board,
        items: &[Copper],
        fills: &[(&GFill, NetId)],
        isl: &[usize],
        wall: &dyn Fn(&GFill) -> bool,
    ) -> RouterBoard {
        let rules = &design.rules;
        let spec = rules.spec();
        let layers = rules.layer_count();
        let all: u64 = rules.layers().0;
        let l0 = LayerId(0);
        let mut nets = design.nets.len();
        for it in items {
            nets = nets.max(it.net.map_or(0, |n| n.index() + 1));
        }
        for (_, n) in fills {
            nets = nets.max(n.index() + 1);
        }
        // A via for the nets that are given none: that of any net that has one.
        let any_via = design.nets.iter().find_map(|s| s.via);
        let keys: Vec<ProfileKey> = (0..nets)
            .map(|i| {
                let net = NetId(i as u32);
                let spec = design.nets.get(i);
                let via = spec.and_then(|s| s.via).or(any_via);
                ProfileKey {
                    width: Nm(spec
                        .map_or(0, |s| s.track_width)
                        .max(rules.track_width(Some(net), l0))),
                    clear: Nm(rules.clearance(Some(net), l0)),
                    via_drill: Nm(via.map_or(0, |v| v.drill)),
                    via_dia: Nm(via.map_or(0, |v| v.diameter)),
                }
            })
            .collect();
        let mut uniq: Vec<ProfileKey> = keys.clone();
        uniq.sort();
        uniq.dedup();
        let profiles: Vec<Profile> = uniq
            .iter()
            .map(|k| Profile {
                key: *k,
                hw: k.width.0 as f64 / 2.0,
                c: k.clear.0 as f64,
                rv: k.via_dia.0 as f64 / 2.0,
                dr: k.via_drill.0 as f64 / 2.0,
                inner: 0.0,
            })
            .collect();
        let net_profile: Vec<usize> = keys
            .iter()
            .map(|k| uniq.binary_search(k).expect("profile"))
            .collect();
        let board_clear = spec.clearance.max(spec.min_clearance) as f64;
        let net_clear = |net: Option<NetId>| -> f64 {
            net.map_or(board_clear, |n| rules.clearance(Some(n), l0) as f64)
        };

        let scoped = {
            let list: Vec<(&GShape, LayerSet, Option<NetId>, Option<i64>)> = items
                .iter()
                .map(|it| (&it.shape, it.layers, it.net, it.own))
                .collect();
            Scoped::new(rules, &list)
        };
        let told = design.parts.len() == board.pads.len();
        let part = |pad: usize| {
            if told {
                format!("P{}", design.parts[pad])
            } else {
                format!("p{pad}")
            }
        };
        let center = |s: &GShape| {
            let b = s.bbox();
            crate::geom::Point::new((b.min().x + b.max().x) / 2, (b.min().y + b.max().y) / 2)
        };
        let mut obstacles = Vec::new();
        let mut terminals = Vec::new();
        for (i, it) in items.iter().enumerate() {
            let shape = Shape::of(&it.shape);
            let net = it.net.map(|n| n.0);
            let (anchor, label, kind, owner) = match it.what {
                What::Pad(p, _) => {
                    let pad = &board.pads[p];
                    let at = P::of(
                        crate::pcb::router::pad_center(&pad.shape)
                            .unwrap_or_else(|| center(&pad.shape)),
                    );
                    // A hole with nothing round it but itself is no pad.
                    let npth = pad.net.is_none() && pad.hole.as_ref() == Some(&pad.shape);
                    (
                        Anchor::Center(at),
                        format!("{}.{p}", part(p)),
                        if npth { ObKind::Npth } else { ObKind::Pad },
                        Some(part(p)),
                    )
                }
                What::Track(t) => {
                    let tr = &board.tracks[t];
                    (
                        Anchor::Segment(P::of(tr.start), P::of(tr.end)),
                        format!("track#{t}"),
                        ObKind::Copper,
                        None,
                    )
                }
                What::Via(v) => (
                    Anchor::Center(P::of(board.vias[v].at)),
                    format!("via#{v}"),
                    ObKind::Copper,
                    None,
                ),
                What::Drawing(z) => (
                    Anchor::Center(P::of(center(&it.shape))),
                    format!("copper#{z}"),
                    ObKind::Copper,
                    None,
                ),
            };
            if let Some(n) = net {
                let at = match anchor {
                    Anchor::Center(c) => c,
                    Anchor::Segment(a, b) => P::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0),
                };
                terminals.push(Terminal {
                    net: n,
                    shape: shape.clone(),
                    layers: it.layers.0,
                    anchor,
                    label,
                    island: isl[i],
                    pad: matches!(it.what, What::Pad(..)),
                    at,
                    what: it.what,
                    item: i,
                });
            }
            obstacles.push(Obstacle {
                bbox: shape.bbox(),
                shape,
                tracks: if kind == ObKind::Npth {
                    all
                } else {
                    it.layers.0
                },
                vias: true,
                net,
                except: Vec::new(),
                clear: Clear::Net(if kind == ObKind::Npth && spec.hole_clearance > 0 {
                    spec.hole_clearance as f64
                } else {
                    net_clear(it.net)
                }),
                kind,
                owner,
                item: (kind != ObKind::Npth).then_some(i as u32),
            });
        }
        // The fills that the run takes as copper before routing (`wall`): copper of their
        // nets to every other net, which no route of another crosses, and to their own a fill
        // as the others are.
        for (f, net) in fills.iter().filter(|(f, _)| wall(f)) {
            let shape = Shape::of(&f.shape);
            obstacles.push(Obstacle {
                bbox: shape.bbox(),
                shape,
                tracks: LayerSet::EMPTY.with(f.layer).0 & all,
                vias: true,
                net: Some(net.0),
                except: Vec::new(),
                // Its zone's own clearance in place of the nets', where that is more than
                // its net's, as the run has it for the copper that it takes the fill as.
                clear: match design.zones.zones.get(f.zone) {
                    Some(z) if z.clearance > rules.clearance(Some(*net), f.layer) => {
                        Clear::Fixed(z.clearance as f64)
                    }
                    _ => Clear::Net(net_clear(Some(*net))),
                },
                kind: ObKind::Copper,
                owner: None,
                item: None,
            });
        }
        // Copper with no net that the board does not hold.
        for (shape, layer) in &design.obstacles {
            let shape = Shape::of(shape);
            obstacles.push(Obstacle {
                bbox: shape.bbox(),
                shape,
                tracks: LayerSet::EMPTY.with(*layer).0 & all,
                vias: true,
                net: None,
                except: Vec::new(),
                clear: Clear::Net(board_clear),
                kind: ObKind::Copper,
                owner: None,
                item: None,
            });
        }
        // Holes.
        let mut holes = Vec::new();
        let capsule = |s: &GShape| -> Shape {
            match Shape::of(s) {
                c @ Shape::Capsule { .. } => c,
                other => {
                    let b = other.bbox();
                    let c = P::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
                    Shape::Capsule {
                        a: c,
                        b: c,
                        r: (b.max.x - b.min.x).min(b.max.y - b.min.y) / 2.0,
                    }
                }
            }
        };
        for (p, pad) in board.pads.iter().enumerate() {
            let Some(hole) = &pad.hole else { continue };
            let shape = capsule(hole);
            let Shape::Capsule { a, b, r } = shape else {
                continue;
            };
            holes.push(Hole {
                at: P::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0),
                r,
            });
            let copper = pad.copper_layers().0 & all != 0
                && !(pad.net.is_none() && pad.hole.as_ref() == Some(&pad.shape));
            if !copper && pad.copper_layers().0 & all == 0 {
                // A hole with no copper: the hole clearance when the rules set one, else the
                // copper clearance; a net's larger clearance still applies.
                obstacles.push(Obstacle {
                    bbox: shape.bbox(),
                    shape,
                    tracks: all,
                    vias: true,
                    net: None,
                    except: Vec::new(),
                    clear: Clear::Net(if spec.hole_clearance > 0 {
                        spec.hole_clearance as f64
                    } else {
                        board_clear
                    }),
                    kind: ObKind::Npth,
                    owner: Some(part(p)),
                    item: None,
                });
            } else if copper && scoped.hc > 0.0 {
                // Plated holes keep the hole clearance from copper of other nets.
                obstacles.push(Obstacle {
                    bbox: shape.bbox(),
                    shape,
                    tracks: all,
                    vias: true,
                    net: pad.net.map(|n| n.0),
                    except: Vec::new(),
                    clear: Clear::Fixed(scoped.hc),
                    kind: ObKind::Copper,
                    owner: Some(part(p)),
                    item: None,
                });
            }
        }
        for v in &board.vias {
            let c = P::of(v.at);
            holes.push(Hole {
                at: c,
                r: v.drill as f64 / 2.0,
            });
            if scoped.hc > 0.0 {
                let shape = Shape::Capsule {
                    a: c,
                    b: c,
                    r: v.drill as f64 / 2.0,
                };
                obstacles.push(Obstacle {
                    bbox: shape.bbox(),
                    shape,
                    tracks: all,
                    vias: true,
                    net: v.net.map(|n| n.0),
                    except: Vec::new(),
                    clear: Clear::Fixed(scoped.hc),
                    kind: ObKind::Copper,
                    owner: None,
                    item: None,
                });
            }
        }
        // Outline: edges keep the edge clearance; inside test on the rings.
        let mut outer: Option<Poly> = None;
        let mut cutouts = Vec::new();
        let mut bbox = BoxF::EMPTY;
        let mut rings: Vec<Poly> = outline_rings(&design.outline)
            .into_iter()
            .map(|r| Poly::new(vec![r]))
            .collect();
        // The largest ring is the board, the others its cutouts.
        let area = |p: &Poly| (p.bbox.max.x - p.bbox.min.x) * (p.bbox.max.y - p.bbox.min.y);
        if let Some(big) = (0..rings.len())
            .max_by(|&a, &b| area(&rings[a]).total_cmp(&area(&rings[b])).then(b.cmp(&a)))
        {
            rings.swap(0, big);
        }
        for poly in rings {
            let ring = &poly.rings[0];
            for e in 0..ring.len() {
                let (a, b) = (ring[e], ring[(e + 1) % ring.len()]);
                let shape = Shape::Capsule { a, b, r: 0.0 };
                obstacles.push(Obstacle {
                    bbox: shape.bbox(),
                    shape,
                    tracks: all,
                    vias: true,
                    net: None,
                    except: Vec::new(),
                    clear: Clear::Fixed(design.edge_clearance as f64),
                    kind: ObKind::Edge,
                    owner: None,
                    item: None,
                });
            }
            if outer.is_none() {
                bbox = poly.bbox;
                outer = Some(poly);
            } else {
                cutouts.push(poly);
            }
        }
        // Keep-outs.
        for k in &board.keepouts {
            if !(k.tracks || k.vias) {
                continue;
            }
            let shape = Shape::of(&k.shape);
            let mut except: Vec<u32> = k.except.iter().map(|n| n.0).collect();
            except.sort_unstable();
            obstacles.push(Obstacle {
                bbox: shape.bbox(),
                shape,
                tracks: if k.tracks { k.layers.0 & all } else { 0 },
                vias: k.vias,
                net: None,
                except,
                clear: Clear::Fixed(0.0),
                kind: ObKind::Keepout,
                owner: None,
                item: None,
            });
        }
        RouterBoard {
            layers,
            net_profile,
            profiles,
            obstacles,
            holes,
            outer,
            cutouts,
            bbox,
            h2h: spec.hole_to_hole as f64,
            rules: BoardRules {
                clearance: board_clear,
                copper_to_edge: Nm(design.edge_clearance),
                min_track_width: Nm(spec.track_width),
                min_drill: Nm(spec.min_drill),
                min_annular_ring: Nm(spec.min_annular_ring),
            },
            terminals,
            fills: fills_of(fills, isl, items.len()),
            scoped,
        }
    }

    /// Whether `pt` lies inside the board (outer contour, outside every cutout).
    pub fn inside(&self, pt: P) -> bool {
        self.outer.as_ref().is_some_and(|o| o.contains(pt))
            && !self.cutouts.iter().any(|c| c.contains(pt))
    }

    /// Profile of a net.
    pub fn profile(&self, net: u32) -> &Profile {
        &self.profiles[self.net_profile[net as usize]]
    }

    // ---- clearances (scoped rules, see `scoped`) ------------------------------------------

    /// Clearance between new copper of profile `pr` in state `st` and obstacle `ob`.
    pub fn c_ob(&self, pr: &Profile, st: St, ob: &Obstacle) -> f64 {
        match ob.item {
            Some(i) if self.scoped.on => self.scoped.resolve(Who::New(st, pr.c), Who::Item(i)),
            _ => ob.clear.with(pr.c),
        }
    }

    /// Clearance between new copper of profiles `a` and `b` (different nets) in states `sa`,
    /// `sb`.
    pub fn c_cu(&self, a: &Profile, sa: St, b: &Profile, sb: St) -> f64 {
        if self.scoped.on {
            self.scoped.resolve(Who::New(sa, a.c), Who::New(sb, b.c))
        } else {
            a.c.max(b.c)
        }
    }

    /// State of a via of profile `pr` at `p` (centered within `r` of it).
    pub fn st_via(&self, pr: &Profile, p: P, r: f64) -> St {
        if !self.scoped.positional() {
            return St::default();
        }
        self.scoped.st(Kind::Via, 0, p, r, pr.rv)
    }

    /// Pieces of a straight track of profile `pr` (half width `hw`) with uniform states.
    pub fn pieces(&self, pr: &Profile, layer: usize, a: P, b: P, hw: f64) -> Vec<(P, P, St)> {
        self.scoped.split(layer, a, b, pr.inner, hw)
    }

    /// Distance from the center of a via of profile `pr` to copper of another net at copper
    /// clearance `c` (hole clearance included).
    pub fn via_out(&self, pr: &Profile, c: f64) -> f64 {
        self.scoped.via_out(pr.rv, pr.dr, c)
    }

    /// Distance between the centers of vias of profiles `a` and `b` of different nets at
    /// copper clearance `c`.
    pub fn via_via(&self, a: &Profile, b: &Profile, c: f64) -> f64 {
        self.scoped.via_via((a.rv, a.dr), (b.rv, b.dr), c, self.h2h)
    }

    /// The largest clearance any check may need (custom rules included).
    pub fn max_clear(&self) -> f64 {
        let mut c = self
            .rules
            .clearance
            .max(self.scoped.max_clear())
            .max(self.scoped.hc);
        for p in &self.profiles {
            c = c.max(p.c);
        }
        c
    }

    /// Whether a track of profile `pr` (its pieces `mine`, with `extra` beyond its half width)
    /// comes closer than allowed to a track of profile `po` (half width `hwo`) from `c` to `d` on
    /// the same `layer`.
    #[allow(clippy::too_many_arguments)]
    pub fn segs_conflict(
        &self,
        pr: &Profile,
        layer: usize,
        mine: &[(P, P, St)],
        extra: f64,
        po: &Profile,
        c: P,
        d: P,
        hwo: f64,
    ) -> bool {
        let (a, b) = (mine[0].0, mine[mine.len() - 1].1);
        if !self.scoped.on {
            return seg_seg_dist(a, b, c, d) < pr.hw + hwo + pr.c.max(po.c) + extra - TOL;
        }
        if !self.scoped.positional() {
            let cl = self.c_cu(pr, St::default(), po, St::default());
            return seg_seg_dist(a, b, c, d) < pr.hw + hwo + cl + extra - TOL;
        }
        let dist = seg_seg_dist(a, b, c, d);
        if dist >= pr.hw + hwo + self.max_clear() + extra {
            return false;
        }
        let theirs = self.pieces(po, layer, c, d, hwo);
        mine.iter().any(|&(pa, pb, sa)| {
            theirs.iter().any(|&(pc, pd, sb)| {
                seg_seg_dist(pa, pb, pc, pd) < pr.hw + hwo + self.c_cu(pr, sa, po, sb) + extra - TOL
            })
        })
    }

    /// Whether a track of profile `pr` (pieces `mine`, `extra` beyond its half width) comes
    /// closer than allowed to a via of profile `po` at `at` (of another net).
    pub fn seg_via_conflict(
        &self,
        pr: &Profile,
        mine: &[(P, P, St)],
        extra: f64,
        po: &Profile,
        at: P,
    ) -> bool {
        let (a, b) = (mine[0].0, mine[mine.len() - 1].1);
        if !self.scoped.on {
            return point_seg_d2(at, a, b).sqrt()
                < pr.hw + self.via_out(po, pr.c.max(po.c)) + extra - TOL;
        }
        if !self.scoped.positional() {
            let cl = self.c_cu(pr, St::default(), po, St::default());
            return point_seg_d2(at, a, b).sqrt() < pr.hw + self.via_out(po, cl) + extra - TOL;
        }
        let dist = point_seg_d2(at, a, b).sqrt();
        if dist >= pr.hw + self.via_out(po, self.max_clear()) + extra {
            return false;
        }
        let sv = self.st_via(po, at, 0.0);
        mine.iter().any(|&(pa, pb, sa)| {
            point_seg_d2(at, pa, pb).sqrt()
                < pr.hw + self.via_out(po, self.c_cu(pr, sa, po, sv)) + extra - TOL
        })
    }

    /// Whether a via of profile `pr` at `at` (state `st`) comes closer than allowed to a track of
    /// profile `po` (half width `hwo`) from `a` to `b` on `layer` (of another net).
    #[allow(clippy::too_many_arguments)]
    pub fn via_seg_conflict(
        &self,
        pr: &Profile,
        at: P,
        st: St,
        po: &Profile,
        layer: usize,
        a: P,
        b: P,
        hwo: f64,
    ) -> bool {
        if !self.scoped.on {
            return point_seg_d2(at, a, b).sqrt() < self.via_out(pr, pr.c.max(po.c)) + hwo - TOL;
        }
        if !self.scoped.positional() {
            let cl = self.c_cu(pr, st, po, St::default());
            return point_seg_d2(at, a, b).sqrt() < self.via_out(pr, cl) + hwo - TOL;
        }
        let dist = point_seg_d2(at, a, b).sqrt();
        if dist >= self.via_out(pr, self.max_clear()) + hwo {
            return false;
        }
        self.pieces(po, layer, a, b, hwo)
            .iter()
            .any(|&(pa, pb, sb)| {
                point_seg_d2(at, pa, pb).sqrt()
                    < self.via_out(pr, self.c_cu(pr, st, po, sb)) + hwo - TOL
            })
    }

    /// State of tracks of profile `pr` anywhere in `bbox` on any of `layers` (all copper
    /// layers when empty).
    pub fn st_track_box(&self, pr: &Profile, layers: &[usize], bbox: &BoxF) -> St {
        if !self.scoped.positional() {
            return St::default();
        }
        let all: Vec<usize>;
        let layers = if layers.is_empty() {
            all = (0..self.layers).collect();
            &all[..]
        } else {
            layers
        };
        self.scoped
            .st_box(Kind::Track, layers, &bbox.expand(pr.inner), pr.hw)
    }

    /// The clearance new tracks of terminal `t`'s net keep near it from the other pads of its
    /// component (the largest; the net's class clearance without scoped rules).
    pub fn c_pad_near(&self, t: usize) -> f64 {
        let term = &self.terminals[t];
        let pr = self.profile(term.net);
        if !self.scoped.on {
            return pr.c;
        }
        let Some((comp, _)) = term.label.split_once('.') else {
            return pr.c;
        };
        let bb = term.shape.bbox().expand(pr.hw + self.max_clear());
        let st = self.st_track_box(pr, &[], &bb);
        let mut c: Option<f64> = None;
        for ob in &self.obstacles {
            if ob.kind == ObKind::Pad
                && ob.owner.as_deref() == Some(comp)
                && ob.net != Some(term.net)
                && ob.bbox.intersects(&bb)
            {
                let v = self.c_ob(pr, st, ob);
                c = Some(c.map_or(v, |x: f64| x.max(v)));
            }
        }
        c.unwrap_or_else(|| self.c_cu(pr, st, pr, st))
    }

    /// The clearance between two new tracks of terminal `t`'s net profile near it (two escapes
    /// side by side).
    pub fn c_tracks_near(&self, t: usize) -> f64 {
        let term = &self.terminals[t];
        let pr = self.profile(term.net);
        if !self.scoped.on {
            return pr.c;
        }
        let bb = term.shape.bbox().expand(pr.hw + self.max_clear());
        let st = self.st_track_box(pr, &[], &bb);
        self.c_cu(pr, st, pr, st)
    }

    /// The narrowest track the custom track width rules allow near terminal `t` (0 when none).
    pub fn min_width_near(&self, t: usize) -> f64 {
        if !self.scoped.has_widths() {
            return 0.0;
        }
        let term = &self.terminals[t];
        let pr = self.profile(term.net);
        let st = self.st_track_box(pr, &[], &term.shape.bbox().expand(pr.hw));
        self.scoped.min_width(st)
    }

    /// The clearance between new tracks of terminal `t`'s net and of net `other` near it.
    pub fn c_nets_near(&self, t: usize, other: u32) -> f64 {
        let term = &self.terminals[t];
        let (pr, po) = (self.profile(term.net), self.profile(other));
        if !self.scoped.on {
            return pr.c.max(po.c);
        }
        let bb = term.shape.bbox().expand(pr.hw + self.max_clear());
        let st = self.st_track_box(pr, &[], &bb);
        self.c_cu(pr, st, po, st)
    }
}
