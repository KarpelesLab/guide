//! Escape routing: getting out of fine-pitch and area-array footprints, before anything
//! else is routed.
//!
//! Pads that are closer to each other than the narrowest track and its clearances on
//! either side, so that nothing passes between them, form a *cluster*, such as a BGA, a
//! QFN or an aQFN with its exposed pad. A pad of a cluster is *trapped* if a track of its
//! net can't get out of the cluster on the pad's layers, past the copper that is already
//! there: an inner ball of a BGA, an inner row of an aQFN, or an exposed pad that the
//! others surround. Pads of one net that a track can join inside the cluster are trapped
//! together, as one *region*, which one via is enough to get out of.
//!
//! Each region gets a through via that its net can reach from one of its pads: in the
//! pad, or beside it at the end of a short track, a dog-bone. The places where one fits
//! are few and close together, a few tens of micrometres wide between an aQFN's rows, too
//! narrow for the lattice that detailed routing moves on to find. So they are searched
//! for on a much finer grid, and chosen so as to leave room for the others: the region with
//! the fewest places left goes first, and takes the place that leaves the most to the
//! region it leaves the fewest to. The vias and tracks chosen become fixed copper of their
//! nets, which detailed routing then completes, from the via on any layer.

use std::collections::VecDeque;

use super::world::{Verdict, World};
use super::{Design, ViaSize};
use crate::geom::drc::{Track, Via};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::NetId;
use crate::space::FastMap;

/// The fewest pads a cluster has, below which it is not a footprint of fine pitch.
const MIN_CLUSTER: usize = 8;

/// How many nodes the search for a way out of a cluster may reach before it is taken to
/// have found one.
const FLOOD_BUDGET: usize = 40_000;

/// How many steps longer than the straight way out of its cluster a pad's way out may be,
/// for the pad not to need an escape: more, and it would take a channel that other pads
/// need, such as round the corner of an aQFN.
const DETOUR: i64 = 4;

/// How many places for a via each region keeps, nearest its pads first.
const MAX_CANDIDATES: usize = 160;

/// The escape of one region: a via, and the track to it from a pad if it isn't in one, or
/// where no via fits, tracks out of the cluster.
#[derive(Clone, Debug)]
pub(crate) struct Escape {
    pub(crate) net: NetId,
    pub(crate) tracks: Vec<Track>,
    pub(crate) via: Option<Via>,
}

/// A place for a region's via.
#[derive(Clone, Debug)]
struct Candidate {
    at: Point,
    /// The track from a pad's centre to it, with its layer, if the via doesn't touch the
    /// pad.
    track: Option<(Point, LayerId)>,
    /// Distance from that pad's centre, squared, which orders the places.
    far: i128,
}

/// Pads of one net trapped in a cluster, that one via gets out of.
struct Region {
    net: NetId,
    /// The pad it was found from, the layers that pad is on, and the box that a way out
    /// of its cluster leaves.
    pad: usize,
    on: LayerSet,
    out: Rect,
    /// The only way out, if no via fits but a track gets out: on a layer, from a pad.
    path: Option<(Vec<Point>, LayerId)>,
    width: i64,
    via: (i64, i64),
    candidates: Vec<Candidate>,
    bbox: Rect,
    done: bool,
    /// The middle of its array, for a ball too deep in one to get out on its layer: its
    /// via goes on that side of it if it can.
    toward: Option<Point>,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (x, y) = (find(parent, a), find(parent, b));
    if x != y {
        parent[x.max(y)] = x.min(y);
    }
}

/// A point inside a shape, its box's centre if that is in it.
fn center_of(shape: &Shape) -> Point {
    let b = shape.bbox();
    Point::new(
        b.min().x + (b.max().x - b.min().x) / 2,
        b.min().y + (b.max().y - b.min().y) / 2,
    )
}

/// Whether fixed copper of a pad's net already touches it, as routing kept from before does:
/// then it is out already.
pub(crate) fn joined(design: &Design, pad: usize) -> bool {
    let b = &design.board;
    let p = &b.pads[pad];
    let Some(net) = p.net else { return false };
    let bbox = p.shape.bbox();
    let tracks = b.tracks.iter().filter(|t| t.net == Some(net)).any(|t| {
        p.layers.contains(t.layer)
            && Rect::new(t.start, t.end).expand(t.width).intersects(&bbox)
            && Shape::track(t.start, t.end, t.width).is_ok_and(|s| s.touches(&p.shape))
    });
    tracks
        || b.vias.iter().filter(|v| v.net == Some(net)).any(|v| {
            Rect::new(v.at, v.at).expand(v.diameter).intersects(&bbox)
                && Shape::circle(v.at, v.diameter).is_ok_and(|s| s.touches(&p.shape))
        })
}

/// Plans the escapes of a design, against the fixed copper of `world`, into which it puts
/// them as routes of their nets as it goes. `pitch` is detailed routing's lattice.
pub(crate) fn plan(
    design: &Design,
    world: &mut World<'_>,
    pitch: i64,
    fanout: Option<ViaSize>,
    arrays: bool,
) -> Vec<Escape> {
    let rules = &design.rules;
    let layers = rules.layer_count();
    if layers < 2 {
        return Vec::new();
    }
    let all = rules.layers();
    let pads = &design.board.pads;
    // Nets with pads to connect: two of them at least.
    let mut count = vec![0usize; design.nets.len()];
    for p in pads {
        if let Some(n) = p.net
            && n.index() < count.len()
        {
            count[n.index()] += 1;
        }
    }
    let routed = |net: Option<NetId>| {
        net.and_then(|n| design.nets.get(n.index()).map(|s| (n, s)))
            .filter(|(n, s)| s.route && count[n.index()] > 1)
    };
    let width_of = |n: NetId, l: LayerId| {
        design.nets[n.index()]
            .track_width
            .max(rules.track_width(Some(n), l))
    };
    // The narrowest track that might pass between two pads, with its clearances.
    let Some(passage) = design
        .nets
        .iter()
        .enumerate()
        .filter(|(_, s)| s.route)
        .map(|(i, s)| {
            let n = Some(NetId(i as u32));
            s.track_width.max(rules.track_width(n, LayerId(0))) + 2 * rules.clearance(n, LayerId(0))
        })
        .min()
    else {
        return Vec::new();
    };

    // Clusters: pads on a common layer with less than a passage between them.
    let copper: Vec<usize> = (0..pads.len())
        .filter(|&i| pads[i].layers.0 & all.0 != 0)
        .collect();
    let index = SpatialIndex::bulk_load(copper.iter().map(|&i| pads[i].shape.bbox()));
    let mut parent: Vec<usize> = (0..copper.len()).collect();
    let mut near = Vec::new();
    for (k, &i) in copper.iter().enumerate() {
        let a = &pads[i];
        index.query(&a.shape.bbox().expand(passage), &mut near);
        for id in &near {
            let m = id.slot() as usize;
            if m <= k {
                continue;
            }
            let b = &pads[copper[m]];
            if a.layers.0 & b.layers.0 & all.0 != 0
                && a.shape.violates(&b.shape, passage) == Ok(true)
            {
                union(&mut parent, k, m);
            }
        }
    }
    let mut members: FastMap<usize, Vec<usize>> = FastMap::default();
    let mut roots = Vec::new();
    for (k, &pad) in copper.iter().enumerate() {
        let r = find(&mut parent, k);
        let list = members.entry(r).or_default();
        if list.is_empty() {
            roots.push(r);
        }
        list.push(pad);
    }

    let mut out = Vec::new();
    // Not for a net with a zone, whose pads go to it by a via where they are, nor for
    // one of many pads, a supply routed as tracks.
    let zoned = |net: NetId| design.zones.zones.iter().any(|z| z.net == Some(net));
    let plain = |net: NetId| !zoned(net) && count[net.index()] <= EXIT_PADS;
    let found = if arrays { balls(design) } else { Vec::new() };
    // The balls deep in an array that have their vias.
    let mut deep_vias: Vec<usize> = Vec::new();
    // In a run with a budget, the ways out of the second ring of an area array first.
    if arrays {
        let usable = |i: usize| {
            let pad = &pads[i];
            routed(pad.net)
                .filter(|(net, _)| !joined(design, i) && plain(*net))
                .map(|(net, _)| net)
        };
        out = ring_exits(design, world, &found, &usable, &width_of);
        // Then the vias of the balls further in whose nets' own via fits beside them.
        if dbg("GUIDE_DBG_AVIAS") {
            let signals_only = !dbg("GUIDE_DBG_AVZ");
            let wanted = |i: usize| {
                routed(pads[i].net)
                    .filter(|(net, _)| !joined(design, i) && !(signals_only && zoned(*net)))
                    .and_then(|(net, spec)| spec.via.map(|v| (net, (v.diameter, v.drill))))
            };
            array_vias(design, world, &found, fanout, &wanted, &width_of, &mut out, &mut deep_vias);
        }
    }
    let mut regions: Vec<Region> = Vec::new();
    // The balls too deep in an area array to get out on their layer, whose nets' own via
    // fits nowhere beside them, so that routing has no way to take them to another layer:
    // each is a region with the places of the smaller via, away from the array's middle.
    let mut deep_pads: Vec<usize> = Vec::new();
    if let (true, Some(small)) = (arrays, fanout) {
        for (i, middle) in deep(&found) {
            if world.run().is_some_and(super::control::Run::poll) {
                return Vec::new();
            }
            if deep_vias.contains(&i) {
                continue;
            }
            let pad = &pads[i];
            let Some((net, spec)) = routed(pad.net) else {
                continue;
            };
            let Some(via) = spec.via else { continue };
            let on = LayerSet(pad.layers.0 & all.0);
            if joined(design, i) || small.diameter >= via.diameter || small.drill > via.drill {
                continue;
            }
            let own = (via.diameter, via.drill);
            if !candidates_of(world, design, net, &[i], own, pitch, &width_of).is_empty() {
                continue;
            }
            let small = (small.diameter, small.drill);
            let candidates = candidates_of(world, design, net, &[i], small, pitch, &width_of);
            if candidates.is_empty() {
                continue;
            }
            deep_pads.push(i);
            regions.push(Region {
                net,
                pad: i,
                on,
                out: pad.shape.bbox(),
                path: None,
                width: width_of(net, on.iter().next().unwrap_or(LayerId(0))),
                via: small,
                candidates,
                bbox: pad.shape.bbox(),
                done: false,
                toward: Some(middle),
            });
        }
    }
    for r in roots {
        let cluster = &members[&r];
        if cluster.len() < MIN_CLUSTER {
            continue;
        }
        let bbox = cluster
            .iter()
            .map(|&i| pads[i].shape.bbox())
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default();
        let out = bbox.expand(passage);
        // By pad, the region it is trapped in.
        let mut region_of: FastMap<usize, usize> = FastMap::default();
        for &i in cluster {
            // A run that has to stop does without escapes: none is placed yet.
            if world.run().is_some_and(super::control::Run::poll) {
                return Vec::new();
            }
            let pad = &pads[i];
            let Some((net, spec)) = routed(pad.net) else {
                continue;
            };
            let Some(via) = spec.via else { continue };
            let on = LayerSet(pad.layers.0 & all.0);
            let whole = LayerSet(pad.copper_layers().0 & all.0);
            if whole == all
                || region_of.contains_key(&i)
                || joined(design, i)
                || deep_pads.contains(&i)
                || deep_vias.contains(&i)
            {
                continue;
            }
            let near_way = flood(world, net, i, design, on, pitch, &out, true, &width_of);
            let Flood::Trapped(reached) = near_way else {
                continue;
            };
            let width = width_of(net, on.iter().next().unwrap_or(LayerId(0)));
            let mut group = vec![i];
            group.extend(reached.iter().copied().filter(|&p| p != i));
            let mut via = (via.diameter, via.drill);
            let mut candidates = candidates_of(world, design, net, &group, via, pitch, &width_of);
            // Where the net's own via fits nowhere, as between the balls of a BGA of fine
            // pitch, the smaller one for fan-outs is tried, if there is one.
            if candidates.is_empty()
                && let Some(small) = fanout.filter(|f| f.diameter < via.0 && f.drill <= via.1)
            {
                let small = (small.diameter, small.drill);
                let found = candidates_of(world, design, net, &group, small, pitch, &width_of);
                if !found.is_empty() {
                    (via, candidates) = (small, found);
                }
            }
            // A pad with a long way out takes it if no via fits, and otherwise needs no
            // escape: routing finds the via if it is the better way. With no way out at
            // all, it is trapped.
            let path = match flood(world, net, i, design, on, pitch, &out, false, &width_of) {
                Flood::Out(path, _) if path.is_empty() || !candidates.is_empty() => continue,
                Flood::Out(path, layer) => Some((path, layer)),
                Flood::Trapped(_) => None,
            };
            // With the pads of its net it reached, it is a region.
            let at = regions.len();
            let mut bbox = pad.shape.bbox();
            for &p in &reached {
                region_of.insert(p, at);
                bbox = bbox.union(&pads[p].shape.bbox());
            }
            region_of.insert(i, at);
            regions.push(Region {
                net,
                pad: i,
                on,
                out,
                path,
                width,
                via,
                candidates,
                bbox,
                done: false,
                toward: None,
            });
        }
    }

    // Regions with no room for a via get out by the shortest way there is, first, as it is
    // the only one.
    for r in regions.iter_mut().filter(|r| r.candidates.is_empty()) {
        r.done = true;
        let Some((mut path, mut layer)) = r.path.take() else {
            continue;
        };
        // Each way out was found before any was placed, and two regions' may be the same
        // channel: one that an earlier region's tracks have taken since is searched for
        // again, round them, and a region with none left gets no escape.
        let free = |world: &World<'_>, path: &[Point], layer: LayerId| {
            path.windows(2).all(|w| {
                Shape::track(w[0], w[1], width_of(r.net, layer))
                    .is_ok_and(|s| world.track(r.net, layer, &s, None) == Verdict::Free)
            })
        };
        if !free(world, &path, layer) {
            match flood(
                world, r.net, r.pad, design, r.on, pitch, &r.out, false, &width_of,
            ) {
                Flood::Out(again, l) if !again.is_empty() => (path, layer) = (again, l),
                _ => continue,
            }
        }
        let width = width_of(r.net, layer);
        let mut tracks: Vec<Track> = Vec::new();
        for w in path.windows(2) {
            let (a, b) = (w[0], w[1]);
            // Steps in line make one track.
            if let Some(t) = tracks.last_mut()
                && (t.end.x - t.start.x).signum() == (b.x - a.x).signum()
                && (t.end.y - t.start.y).signum() == (b.y - a.y).signum()
            {
                t.end = b;
                continue;
            }
            tracks.push(Track {
                start: a,
                end: b,
                mid: None,
                width,
                layer,
                net: Some(r.net),
            });
        }
        for t in &tracks {
            if let Ok(shape) = Shape::track(t.start, t.end, t.width) {
                world.add_track(r.net, t.layer, shape);
            }
        }
        out.push(Escape {
            net: r.net,
            tracks,
            via: None,
        });
    }
    loop {
        // A run that has to stop keeps the escapes placed so far.
        if world.run().is_some_and(super::control::Run::poll) {
            break;
        }
        // The places still free, for each region left.
        for r in regions.iter_mut().filter(|r| !r.done) {
            let (net, (d, drill), width) = (r.net, r.via, r.width);
            r.candidates
                .retain(|c| still_free(world, net, c, d, drill, width));
        }
        let Some(r) = (0..regions.len())
            .filter(|&r| !regions[r].done && !regions[r].candidates.is_empty())
            .min_by_key(|&r| {
                // The balls deep in an array first, the furthest from its middle first:
                // each has one place on the side away from the middle, which a ball
                // nearer the middle would otherwise take.
                let far = regions[r].toward.map(|m| {
                    let c = center_of(&pads[regions[r].pad].shape);
                    let (dx, dy) = ((c.x - m.x) as i128, (c.y - m.y) as i128);
                    core::cmp::Reverse(dx * dx + dy * dy)
                });
                (far.is_none(), far, regions[r].candidates.len(), r)
            })
        else {
            break;
        };
        let reach = regions[r].via.0 * 2 + passage;
        let neighbours: Vec<usize> = (0..regions.len())
            .filter(|&q| {
                q != r
                    && !regions[q].done
                    && regions[q].bbox.intersects(&regions[r].bbox.expand(reach))
            })
            .collect();
        // The place that leaves the most to the neighbour it leaves the least, then takes
        // the fewest from all of them, then is nearest its pad.
        let mut best: Option<((bool, usize, usize, i128), usize)> = None;
        let mut stopped = false;
        for (k, c) in regions[r].candidates.iter().enumerate() {
            if world.run().is_some_and(super::control::Run::poll) {
                stopped = true;
                break;
            }
            let mut least = usize::MAX;
            let mut taken = 0;
            for &q in &neighbours {
                let left = regions[q]
                    .candidates
                    .iter()
                    .filter(|o| !clash(design, &regions[r], c, &regions[q], o))
                    .count();
                taken += regions[q].candidates.len() - left;
                least = least.min(left);
            }
            // For a ball deep in an array, a place on the side away from the array's
            // middle before any other, as a board routed by hand has them: the vias of
            // each quarter of the array then stand in rows that the tracks of the other
            // layers leave it between, outwards, and the rows through its middle are free
            // of them. The ways out on the balls' own layer, between the balls of the
            // first ring, are those of the second ring's balls, planned before.
            // Towards the middle, `glasgow-revC3` had 24 to 29 connections open in
            // four orders at a minute where it has 4 to 7 (`docs/benchmarks.md`).
            let inward = regions[r].toward.is_some_and(|m| {
                let from = center_of(&pads[regions[r].pad].shape);
                let side =
                    |at: i64, from: i64, to: i64| (at - from).signum() * (to - from).signum();
                side(c.at.x, from.x, m.x) > 0 || side(c.at.y, from.y, m.y) > 0
            });
            let key = (inward, usize::MAX - least, taken, c.far);
            if best.as_ref().is_none_or(|(b, _)| key < *b) {
                best = Some((key, k));
            }
        }
        if stopped {
            break;
        }
        let Some((_, k)) = best else { break };
        let region = &mut regions[r];
        region.done = true;
        let c = region.candidates[k].clone();
        let (diameter, drill) = region.via;
        let track = c.track.map(|(from, layer)| Track {
            start: from,
            end: c.at,
            mid: None,
            width: region.width,
            layer,
            net: Some(region.net),
        });
        if let Some(t) = &track
            && let Ok(shape) = Shape::track(t.start, t.end, t.width)
        {
            world.add_track(region.net, t.layer, shape);
        }
        world.add_via(region.net, c.at, diameter, drill);
        out.push(Escape {
            net: region.net,
            tracks: track.into_iter().collect(),
            via: Some(Via {
                at: c.at,
                drill,
                diameter,
                from: LayerId(0),
                to: LayerId((layers - 1) as u8),
                net: Some(region.net),
            }),
        });
    }
    out
}

/// How many pads at most a net has for a ball of it in the second ring of an array to
/// be given its way out.
const EXIT_PADS: usize = 8;

/// How many tracks at most pass between two balls for them to be of an array whose
/// inner balls are looked at.
const ARRAY_TRACKS: i64 = 3;

/// The balls of area arrays that are too deep to get out on their layer, each with the
/// middle of its array. An array is pads of one size with no hole, on one layer, in rows
/// both ways, with room for a few tracks at most between two of them. Between two balls
/// that `n` tracks of a ball's net pass, the balls of the outermost `n + 1` rings get
/// out, each ring through the ways between the balls of the rings outside it, and no ball
/// further in does, whatever is routed first: its depth, in balls from the nearest one
/// with none beside it on some side, is more than `n`.
fn deep(balls: &[Ball]) -> Vec<(usize, Point)> {
    balls
        .iter()
        .filter(|b| b.depth > b.tracks)
        .map(|b| (b.pad, b.middle))
        .collect()
}

/// A switch for what is being tried, on unless the environment says 0.
fn dbg(name: &str) -> bool {
    std::env::var(name).map_or(true, |v| v != "0")
}

/// How many balls too deep to get out on their layer an array has for its vias and
/// their ways out to be planned.
const DEEP_ARRAY: usize = 12;

/// The step between two balls of an array, by the balls beside one.
fn step_of(b: &Ball) -> Option<i64> {
    b.beside
        .iter()
        .flatten()
        .map(|&(at, _)| (at.x - b.at.x).abs().max((at.y - b.at.y).abs()))
        .min()
}

/// The vias of the balls too deep in an array to get out on their layer, where the
/// net's own via fits beside the ball: in the middle of four balls, on the side away
/// from the array's middle if it is free, at the end of a track from the ball, as a board
/// routed by hand has them. Placed before anything is routed, the vias stand in rows
/// with the lines of the balls free between them on every other layer; found by each
/// net's search for itself, one is wherever that net's way was shortest, and the nets
/// after it have no way through. The smaller via for fan-outs is used where a track of
/// the net passes between two of those side by side and not between two of its own. For
/// the nets with zones too, whose balls need no more than that via. Into `world` and
/// `out`, with each ball's place in `out` in `placed`.
#[allow(clippy::too_many_arguments)]
fn array_vias(
    design: &Design,
    world: &mut World<'_>,
    balls: &[Ball],
    fanout: Option<ViaSize>,
    wanted: &dyn Fn(usize) -> Option<(NetId, (i64, i64))>,
    width_of: &dyn Fn(NetId, LayerId) -> i64,
    out: &mut Vec<Escape>,
    placed: &mut Vec<usize>,
) {
    let rules = &design.rules;
    let layers = rules.layer_count();
    let mut deep: Vec<&Ball> = balls.iter().filter(|b| b.depth > b.tracks).collect();
    // The outermost first, so that a ball nearer the middle finds theirs in place.
    deep.sort_by_key(|b| (b.middle.x, b.middle.y, b.layer.0, b.depth, b.at.y, b.at.x));
    // The four places beside a ball: away from the middle both ways first, then one
    // way, then towards it.
    let places = |b: &Ball, step: i64| {
        let away = |at: i64, middle: i64| if at >= middle { 1 } else { -1 };
        let (ax, ay) = (away(b.at.x, b.middle.x), away(b.at.y, b.middle.y));
        let half = step / 2;
        [(ax, ay), (ax, -ay), (-ax, ay), (-ax, -ay)]
            .map(|(sx, sy)| Point::new(b.at.x + sx * half, b.at.y + sy * half))
    };
    let fits = |world: &World<'_>, b: &Ball, net: NetId, at: Point, (d, drill): (i64, i64)| {
        world.via(net, at, d, drill, None) == Verdict::Free
            && Shape::track(b.at, at, width_of(net, b.layer))
                .is_ok_and(|s| world.track(net, b.layer, &s, None) == Verdict::Free)
    };
    // The arrays that this is for: those with balls enough that deep, most of which
    // have a place for their net's own via beside them before any is placed. One whose
    // balls have none is the smaller via's, further on.
    let mut arrays: Vec<(Point, LayerId)> = Vec::new();
    for b in &deep {
        let key = (b.middle, b.layer);
        if arrays.contains(&key) || world.run().is_some_and(super::control::Run::poll) {
            continue;
        }
        let of_it = deep.iter().filter(|o| (o.middle, o.layer) == key);
        let with_place = of_it
            .clone()
            .filter(|o| {
                wanted(o.pad).zip(step_of(o)).is_some_and(|((net, own), step)| {
                    places(o, step).iter().any(|&at| fits(world, o, net, at, own))
                })
            })
            .count();
        let all = of_it.count();
        if all >= DEEP_ARRAY && 2 * with_place > all {
            arrays.push(key);
        }
    }
    for b in &deep {
        if world.run().is_some_and(super::control::Run::poll) {
            return;
        }
        if !arrays.contains(&(b.middle, b.layer)) {
            continue;
        }
        let (Some((net, own)), Some(step)) = (wanted(b.pad), step_of(b)) else {
            continue;
        };
        let width = width_of(net, b.layer);
        let clearance = rules.clearance(Some(net), b.layer);
        // Whether a track of the net passes between two such vias side by side.
        let room = |(d, drill): (i64, i64)| {
            step - d - 2 * clearance >= width
                && step - drill - 2 * rules.spec().hole_clearance >= width
        };
        let small = fanout
            .map(|f| (f.diameter, f.drill))
            .filter(|f| f.0 < own.0 && f.1 <= own.1);
        let sizes: Vec<(i64, i64)> = match small {
            Some(s) if !room(own) && room(s) => vec![s, own],
            _ => vec![own],
        };
        let places = places(b, step);
        // Only where the net's own via has a place: a ball that it has none beside is
        // one of those that the smaller via is for, further on.
        if !places.iter().any(|&at| fits(world, b, net, at, own)) {
            continue;
        }
        let Some((at, (diameter, drill))) = sizes.iter().find_map(|&v| {
            places
                .iter()
                .find(|&&at| fits(world, b, net, at, v))
                .map(|&at| (at, v))
        }) else {
            continue;
        };
        if let Ok(shape) = Shape::track(b.at, at, width) {
            world.add_track(net, b.layer, shape);
        }
        world.add_via(net, at, diameter, drill);
        out.push(Escape {
            net,
            tracks: vec![Track {
                start: b.at,
                end: at,
                mid: None,
                width,
                layer: b.layer,
                net: Some(net),
            }],
            via: Some(Via {
                at,
                drill,
                diameter,
                from: LayerId(0),
                to: LayerId((layers - 1) as u8),
                net: Some(net),
            }),
        });
        placed.push(b.pad);
    }
}

/// A ball of an area array.
#[derive(Clone, Copy, Debug)]
struct Ball {
    pad: usize,
    at: Point,
    layer: LayerId,
    /// In balls from the nearest one with none beside it on some side.
    depth: i64,
    /// How many tracks of its net pass between it and the nearest ball beside it.
    tracks: i64,
    /// The middle of its array.
    middle: Point,
    /// The balls beside it, right, above, left and below: where each is, and whether it
    /// is the last of the array that way.
    beside: [Option<(Point, bool)>; 4],
}

/// The balls of area arrays whose nets are routed: an array is pads of one size with no
/// hole, on one layer, in rows both ways, with room for a few tracks at most between two
/// of them.
fn balls(design: &Design) -> Vec<Ball> {
    let rules = &design.rules;
    let stack = rules.layers();
    let pads = &design.board.pads;
    // The pads on one layer, with their centres and sizes.
    let single: Vec<(usize, Point, LayerId, i64)> = (0..pads.len())
        .filter_map(|i| {
            let p = &pads[i];
            let on = LayerSet(p.copper_layers().0 & stack.0);
            let mut layers = on.iter();
            let layer = layers.next()?;
            if p.hole.is_some() || layers.next().is_some() {
                return None;
            }
            let b = p.shape.bbox();
            let (w, h) = (b.max().x - b.min().x, b.max().y - b.min().y);
            ((w - h).abs() <= 2_000).then_some((i, center_of(&p.shape), layer, w.max(h)))
        })
        .collect();
    // The most room between two balls of an array: a few tracks of the narrowest net.
    let Some(room) = design
        .nets
        .iter()
        .enumerate()
        .filter(|(_, s)| s.route)
        .map(|(i, s)| {
            let n = Some(NetId(i as u32));
            let (w, c) = (
                s.track_width.max(rules.track_width(n, LayerId(0))),
                rules.clearance(n, LayerId(0)),
            );
            ARRAY_TRACKS * (w + c) + c
        })
        .min()
    else {
        return Vec::new();
    };
    let index = SpatialIndex::bulk_load(single.iter().map(|s| pads[s.0].shape.bbox()));
    // By ball, the balls beside it: right, above, left, below.
    let mut beside: Vec<[Option<usize>; 4]> = vec![[None; 4]; single.len()];
    let mut near = Vec::new();
    for (k, &(i, at, layer, size)) in single.iter().enumerate() {
        index.query(&pads[i].shape.bbox().expand(room + 2_000), &mut near);
        for id in &near {
            let m = id.slot() as usize;
            let (_, other, on, other_size) = single[m];
            if m == k || on != layer || (other_size - size).abs() > 2_000 {
                continue;
            }
            let (dx, dy) = (other.x - at.x, other.y - at.y);
            let side = match (dx.abs() <= 2_000, dy.abs() <= 2_000) {
                (false, true) if dx > 0 => 0,
                (true, false) if dy > 0 => 1,
                (false, true) => 2,
                (true, false) => 3,
                _ => continue,
            };
            // The nearest that way.
            let far = |m: usize| {
                let q = single[m].1;
                (q.x - at.x).abs().max((q.y - at.y).abs())
            };
            if beside[k][side].is_none_or(|old| (far(m), m) < (far(old), old)) {
                beside[k][side] = Some(m);
            }
        }
    }
    // The balls of arrays: with one beside them both ways.
    let member = |k: usize| {
        let b = &beside[k];
        (b[0].is_some() || b[2].is_some()) && (b[1].is_some() || b[3].is_some())
    };
    // Depth, from the balls with none beside them on some side, and the array, as the
    // first of its balls.
    let mut depth: Vec<Option<i64>> = vec![None; single.len()];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for k in 0..single.len() {
        if member(k) && beside[k].iter().any(|b| b.is_none_or(|m| !member(m))) {
            depth[k] = Some(0);
            queue.push_back(k);
        }
    }
    while let Some(k) = queue.pop_front() {
        for m in beside[k].into_iter().flatten() {
            if member(m) && depth[m].is_none() {
                depth[m] = depth[k].map(|d| d + 1);
                queue.push_back(m);
            }
        }
    }
    let mut array: Vec<Option<usize>> = vec![None; single.len()];
    let mut boxes: FastMap<usize, Rect> = FastMap::default();
    for k in 0..single.len() {
        if !member(k) || array[k].is_some() {
            continue;
        }
        array[k] = Some(k);
        let mut around = Rect::new(single[k].1, single[k].1);
        queue.push_back(k);
        while let Some(j) = queue.pop_front() {
            around = around.union(&Rect::new(single[j].1, single[j].1));
            for m in beside[j].into_iter().flatten() {
                if member(m) && array[m].is_none() {
                    array[m] = Some(k);
                    queue.push_back(m);
                }
            }
        }
        boxes.insert(k, around);
    }
    let mut out: Vec<Ball> = Vec::new();
    for (k, &(i, at, layer, size)) in single.iter().enumerate() {
        let (Some(d), Some(a)) = (depth[k], array[k]) else {
            continue;
        };
        let Some(net) = pads[i].net else { continue };
        let Some(spec) = design.nets.get(net.index()).filter(|s| s.route) else {
            continue;
        };
        // How many tracks of its net pass between it and the nearest ball beside it.
        let gap = beside[k]
            .iter()
            .flatten()
            .map(|&m| {
                let q = single[m].1;
                (q.x - at.x).abs().max((q.y - at.y).abs()) - size
            })
            .min()
            .unwrap_or(0);
        let (w, c) = (
            spec.track_width.max(rules.track_width(Some(net), layer)),
            rules.clearance(Some(net), layer),
        );
        let tracks = ((gap - c) / (w + c).max(1)).max(0);
        let b = boxes[&a];
        let middle = Point::new(
            b.min().x + (b.max().x - b.min().x) / 2,
            b.min().y + (b.max().y - b.min().y) / 2,
        );
        let mut sides = [None; 4];
        for (side, m) in beside[k].iter().enumerate() {
            if let Some(m) = m.filter(|&m| member(m)) {
                let last = beside[m][side].is_none_or(|n| !member(n));
                sides[side] = Some((single[m].1, last));
            }
        }
        out.push(Ball {
            pad: i,
            at,
            layer,
            depth: d,
            tracks,
            middle,
            beside: sides,
        });
    }
    out
}

/// The ways out of the second ring of the area arrays that one track passes between two
/// balls of: for each of its balls, a track on its layer to the middle of the four balls
/// round one of the two ways between the balls outside it, and from there straight out of
/// the array, where that is free in `world`, into which it is put. The first ring's balls
/// get out any way, and a ball further in gets out by a via. A ball of the second ring
/// has those two ways alone, each of which a track that runs along the first ring closes
/// for it and for every ball beside it: such a track is the shortest way from one end of
/// the array to the other, and a search takes it. With every way out taken by its ball
/// first, there is no room for one.
fn ring_exits(
    design: &Design,
    world: &mut World<'_>,
    balls: &[Ball],
    usable: &dyn Fn(usize) -> Option<NetId>,
    width_of: &dyn Fn(NetId, LayerId) -> i64,
) -> Vec<Escape> {
    let rules = &design.rules;
    let mut out = Vec::new();
    let mut found = balls.to_vec();
    found.sort_by_key(|b| (b.at.y, b.at.x));
    for b in found.iter().filter(|b| b.depth == 1 && b.tracks == 1) {
        let Some(net) = usable(b.pad) else { continue };
        let width = width_of(net, b.layer);
        let reach = {
            let shape = design.board.pads[b.pad].shape.bbox();
            (shape.max().x - shape.min().x) / 2 + rules.clearance(Some(net), b.layer) + width
        };
        // Out past the ball beside it that is the last one that way, to either side of it.
        let mut ways: Vec<[Point; 3]> = Vec::new();
        for (side, beside) in b.beside.iter().enumerate() {
            let Some((at, true)) = *beside else { continue };
            let (ux, uy) = ((at.x - b.at.x).signum(), (at.y - b.at.y).signum());
            let pitch = (at.x - b.at.x).abs().max((at.y - b.at.y).abs());
            let half = pitch / 2;
            // Round the array one way first, so that the balls of a side take turns.
            let turns = if side < 2 { [1, -1] } else { [-1, 1] };
            for t in turns {
                let (vx, vy) = (-uy * t, ux * t);
                let mid = Point::new(b.at.x + (ux + vx) * half, b.at.y + (uy + vy) * half);
                let end = Point::new(
                    b.at.x + ux * (pitch + reach) + vx * half,
                    b.at.y + uy * (pitch + reach) + vy * half,
                );
                ways.push([b.at, mid, end]);
            }
        }
        let free = |world: &World<'_>, way: &[Point; 3]| {
            way.windows(2).all(|w| {
                Shape::track(w[0], w[1], width)
                    .is_ok_and(|s| world.track(net, b.layer, &s, None) == Verdict::Free)
            })
        };
        let Some(way) = ways.into_iter().find(|w| free(world, w)) else {
            continue;
        };
        let tracks: Vec<Track> = way
            .windows(2)
            .map(|w| Track {
                start: w[0],
                end: w[1],
                mid: None,
                width,
                layer: b.layer,
                net: Some(net),
            })
            .collect();
        for t in &tracks {
            if let Ok(shape) = Shape::track(t.start, t.end, t.width) {
                world.add_track(net, t.layer, shape);
            }
        }
        out.push(Escape {
            net,
            tracks,
            via: None,
        });
    }
    out
}

/// What a search for a way out of a cluster found.
enum Flood {
    /// A way out, from the pad's centre, on a layer.
    Out(Vec<Point>, LayerId),
    /// None, and the pads of the net that it reached, which are trapped with the pad.
    Trapped(Vec<usize>),
}

/// Searches for a way out of a cluster's box `out` from pad `start`, on its layers and
/// without vias, on a lattice from its centre, breadth first, so that the way found is a
/// shortest. With `near`, only for a way at most [`DETOUR`] steps longer than the straight
/// way out, which a pad needs no escape for.
#[allow(clippy::too_many_arguments)]
fn flood(
    world: &World<'_>,
    net: NetId,
    start: usize,
    design: &Design,
    on: LayerSet,
    pitch: i64,
    out: &Rect,
    near: bool,
    width_of: &dyn Fn(NetId, LayerId) -> i64,
) -> Flood {
    type Node = (i64, i64, u8);
    let pads = &design.board.pads;
    let own: Vec<usize> = (0..pads.len())
        .filter(|&i| pads[i].net == Some(net) && pads[i].layers.0 & on.0 != 0)
        .collect();
    let c = center_of(&pads[start].shape);
    let straight = (c.x - out.min().x)
        .min(out.max().x - c.x)
        .min(c.y - out.min().y)
        .min(out.max().y - c.y)
        .max(0);
    let depth = if near {
        straight / pitch.max(1) + 1 + DETOUR
    } else {
        i64::MAX
    };
    let at = |(i, j, _): Node| Point::new(c.x + i * pitch, c.y + j * pitch);
    // By node reached, the node it was reached from.
    let mut seen: FastMap<Node, Node> = FastMap::default();
    let mut queue: VecDeque<(Node, i64)> = VecDeque::new();
    for l in on.iter() {
        seen.insert((0, 0, l.0), (0, 0, l.0));
        queue.push_back(((0, 0, l.0), 0));
    }
    let mut reached: Vec<usize> = Vec::new();
    while let Some((node, d)) = queue.pop_front() {
        let p = at(node);
        if p.x < out.min().x || p.y < out.min().y || p.x > out.max().x || p.y > out.max().y {
            let mut path = vec![p];
            let mut k = node;
            while k.0 != 0 || k.1 != 0 {
                k = seen[&k];
                path.push(at(k));
            }
            path.reverse();
            return Flood::Out(path, LayerId(node.2));
        }
        if seen.len() > FLOOD_BUDGET {
            return Flood::Out(Vec::new(), LayerId(node.2));
        }
        if d >= depth {
            continue;
        }
        let layer = LayerId(node.2);
        let width = width_of(net, layer);
        for (di, dj) in [
            (1, 0),
            (0, 1),
            (-1, 0),
            (0, -1),
            (1, 1),
            (-1, 1),
            (-1, -1),
            (1, -1),
        ] {
            let key = (node.0 + di, node.1 + dj, node.2);
            if seen.contains_key(&key) {
                continue;
            }
            let q = at(key);
            let legal = Shape::track(p, q, width)
                .is_ok_and(|s| world.track(net, layer, &s, None) == Verdict::Free);
            if !legal {
                continue;
            }
            seen.insert(key, node);
            queue.push_back((key, d + 1));
            let Ok(end) = Shape::circle(q, 2) else {
                continue;
            };
            for &k in &own {
                if !reached.contains(&k)
                    && pads[k].layers.contains(layer)
                    && pads[k].shape.bbox().intersects(&Rect::new(q, q))
                    && pads[k].shape.touches(&end)
                {
                    reached.push(k);
                }
            }
        }
    }
    reached.sort_unstable();
    Flood::Trapped(reached)
}

/// The places for a via of `net` that a pad of `group` reaches, nearest a pad first.
fn candidates_of(
    world: &World<'_>,
    design: &Design,
    net: NetId,
    group: &[usize],
    (diameter, drill): (i64, i64),
    pitch: i64,
    width_of: &dyn Fn(NetId, LayerId) -> i64,
) -> Vec<Candidate> {
    let all = design.rules.layers();
    // Fine enough to find the gaps of a few tens of micrometres between an aQFN's rows.
    let step = (pitch / 16).max(1000);
    let mut out: Vec<Candidate> = Vec::new();
    for &i in group {
        let pad = &design.board.pads[i];
        let c = center_of(&pad.shape);
        let on = LayerSet(pad.layers.0 & all.0);
        let b = pad.shape.bbox().expand(diameter);
        let mut x = b.min().x;
        while x <= b.max().x {
            // A run that has to stop looks no further: its escapes are given up.
            if world.run().is_some_and(super::control::Run::poll) {
                return Vec::new();
            }
            let mut y = b.min().y;
            while y <= b.max().y {
                let at = Point::new(x, y);
                y += step;
                if world.via(net, at, diameter, drill, None) != Verdict::Free {
                    continue;
                }
                let Ok(copper) = Shape::circle(at, diameter) else {
                    continue;
                };
                let track = if pad.shape.touches(&copper) {
                    None
                } else {
                    let Some(l) = on.iter().find(|&l| {
                        Shape::track(c, at, width_of(net, l))
                            .is_ok_and(|s| world.track(net, l, &s, None) == Verdict::Free)
                    }) else {
                        continue;
                    };
                    Some((c, l))
                };
                let (dx, dy) = ((at.x - c.x) as i128, (at.y - c.y) as i128);
                out.push(Candidate {
                    at,
                    track,
                    far: dx * dx + dy * dy,
                });
            }
            x += step;
        }
    }
    out.sort_by_key(|c| (c.far, c.at));
    out.dedup_by_key(|c| c.at);
    out.truncate(MAX_CANDIDATES);
    out
}

/// Whether a place is still free, now that other escapes are in the world.
fn still_free(
    world: &World<'_>,
    net: NetId,
    c: &Candidate,
    d: i64,
    drill: i64,
    width: i64,
) -> bool {
    world.via(net, c.at, d, drill, None) == Verdict::Free
        && c.track.is_none_or(|(from, l)| {
            Shape::track(from, c.at, width)
                .is_ok_and(|s| world.track(net, l, &s, None) == Verdict::Free)
        })
}

/// Whether two regions' escapes at these places can't both be taken.
fn clash(design: &Design, ra: &Region, a: &Candidate, rb: &Region, b: &Candidate) -> bool {
    let rules = &design.rules;
    let spec = rules.spec();
    let gap = |l: LayerId| rules.clearance_between(Some(ra.net), Some(rb.net), l);
    let (Ok(va), Ok(vb)) = (Shape::circle(a.at, ra.via.0), Shape::circle(b.at, rb.via.0)) else {
        return true;
    };
    let (Ok(ha), Ok(hb)) = (Shape::circle(a.at, ra.via.1), Shape::circle(b.at, rb.via.1)) else {
        return true;
    };
    let hit = |x: &Shape, y: &Shape, clearance: i64| x.violates(y, clearance).unwrap_or(true);
    let top = LayerId(0);
    if hit(&va, &vb, gap(top))
        || (spec.hole_to_hole > 0 && hit(&ha, &hb, spec.hole_to_hole))
        || hit(&ha, &vb, spec.hole_clearance)
        || hit(&va, &hb, spec.hole_clearance)
    {
        return true;
    }
    let track = |r: &Region, c: &Candidate| {
        c.track
            .and_then(|(from, l)| Shape::track(from, c.at, r.width).ok().map(|s| (s, l)))
    };
    if let Some((t, l)) = track(ra, a)
        && (hit(&t, &vb, gap(l)) || hit(&t, &hb, spec.hole_clearance))
    {
        return true;
    }
    if let Some((t, l)) = track(rb, b)
        && (hit(&t, &va, gap(l)) || hit(&t, &ha, spec.hole_clearance))
    {
        return true;
    }
    if let (Some((s, l)), Some((t, m))) = (track(ra, a), track(rb, b))
        && l == m
        && hit(&s, &t, gap(l))
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::tests::{MM, design, smd};
    use super::super::world::World;
    use super::super::{Design, RouteOptions, ViaSize};
    use super::{balls, deep, plan};
    use crate::geom::drc::{self, Pad};
    use crate::geom::{Point, RuleSpec, Rules, Shape};

    /// A grid of `n` by `n` balls of 0.35 mm at a pitch of 0.8 mm, each of a net with a
    /// pad outside, tracks of 0.15 mm with as much between them, and holes that copper
    /// keeps 0.25 mm from: one track passes between two balls, and the nets' via of
    /// 0.6 mm fits nowhere among them.
    fn array(n: i64) -> Design {
        let mut pads = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let net = (i * n + j) as u32 + 1;
                let (x, y) = (i * 800_000, j * 800_000);
                pads.push(Pad {
                    shape: Shape::circle(Point::new(x, y), 350_000).unwrap(),
                    ..smd(x, y, net)
                });
                pads.push(smd((i * n + j) * 1_500_000 - 20 * MM, -6 * MM, net));
            }
        }
        let mut d = design(2, pads, (n * n) as usize + 1);
        let mut spec = RuleSpec::new(&["F.Cu", "B.Cu"], 150_000, 150_000);
        spec.hole_clearance = 250_000;
        spec.hole_to_hole = 250_000;
        d.rules = Rules::new(spec).unwrap();
        for net in &mut d.nets {
            net.track_width = 150_000;
        }
        d
    }

    fn world_of(d: &Design) -> World<'_> {
        let mut world = World::new(
            &d.board,
            &d.rules,
            &d.outline,
            d.edge_clearance,
            &d.obstacles,
        )
        .unwrap();
        let options = RouteOptions {
            vias_in_pads: false,
            ..RouteOptions::default()
        };
        world.configure(d, &options);
        world
    }

    #[test]
    fn the_balls_too_deep_in_an_array_get_a_via_away_from_its_middle() {
        let d = array(7);
        // Two rings get out on their layer, the outer one and the one that goes between
        // its balls: the nine balls inside them do not.
        let middle = Point::new(2_400_000, 2_400_000);
        let found = deep(&balls(&d));
        assert_eq!(found.len(), 9);
        assert!(found.iter().all(|&(_, m)| m == middle));
        let small = ViaSize {
            diameter: 300_000,
            drill: 200_000,
        };
        let escapes = plan(&d, &mut world_of(&d), 150_000, Some(small), true);
        let (exits, escapes): (Vec<_>, Vec<_>) = escapes.iter().partition(|e| e.via.is_none());
        assert_eq!(escapes.len(), 9);
        let mut outward = 0;
        for e in &escapes {
            let via = e.via.as_ref().expect("a via");
            assert_eq!((via.diameter, via.drill), (300_000, 200_000));
            // A track from its ball to the via.
            let [track] = &e.tracks[..] else {
                panic!("{:?}", e.tracks)
            };
            let from = track.start;
            assert_eq!(track.end, via.at);
            // The four balls at the corners of the nine have theirs on the side away from
            // the middle, so that the vias of the four quarters leave the rows through
            // the middle free of them on the other layers.
            let side = |at: i64, from: i64, to: i64| (at - from).signum() * (to - from).signum();
            let away =
                side(via.at.x, from.x, middle.x) <= 0 && side(via.at.y, from.y, middle.y) <= 0;
            if from.x != middle.x && from.y != middle.y {
                assert!(away, "{via:?} from {from:?}");
            }
            outward += usize::from(away && from != middle);
        }
        assert!(outward >= 4, "{outward}");
        // The sixteen balls of the second ring have their ways out between the balls of
        // the first, each its own, before any via is placed.
        assert_eq!(exits.len(), 16);
        for e in &exits {
            let [first, second] = &e.tracks[..] else {
                panic!("{:?}", e.tracks)
            };
            let ring = |p: Point| {
                [p.x, p.y]
                    .into_iter()
                    .map(|c| (c / 800_000).min(6 - c / 800_000))
                    .min()
            };
            assert_eq!(first.start.x % 800_000, 0);
            assert_eq!(first.start.y % 800_000, 0);
            assert_eq!(ring(first.start), Some(1), "{first:?}");
            assert_eq!(first.end, second.start);
            // Its end is past the first ring's pads by a track and its clearance.
            let out = |c: i64| (-c).max(c - 4_800_000);
            assert_eq!(
                out(second.end.x).max(out(second.end.y)),
                475_000,
                "{second:?}"
            );
        }
        // They break no rule, with each other or the balls.
        let mut board = d.board.clone();
        for e in exits.iter().chain(&escapes) {
            board.tracks.extend(e.tracks.iter().cloned());
            board.vias.extend(e.via.clone());
        }
        let broken = drc::check(&board, &d.rules).unwrap();
        assert!(broken.is_empty(), "{broken:?}");
        // Not asked for, there are none: no ball is trapped while nothing is routed. With
        // no smaller via to give the nine, the second ring has its ways out all the same.
        assert!(plan(&d, &mut world_of(&d), 150_000, Some(small), false).is_empty());
        let without = plan(&d, &mut world_of(&d), 150_000, None, true);
        assert_eq!(without.len(), 16);
        assert!(without.iter().all(|e| e.via.is_none()));
        // An array of five by five has one ball that deep, and one of four by four none.
        assert_eq!(deep(&balls(&array(5))).len(), 1);
        assert!(deep(&balls(&array(4))).is_empty());
    }

    /// A grid of `n` by `n` balls of 0.32 mm at a pitch of 0.8 mm, each of a net with a
    /// pad outside, tracks of 0.2 mm, 0.1 mm between copper and 0.16 mm from a hole to
    /// copper, on four layers: one track passes between two balls, the nets' via of
    /// 0.45 mm fits in the middle of four of them, and no track passes between two such
    /// vias side by side.
    fn roomy(n: i64) -> Design {
        let mut pads = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let net = (i * n + j) as u32 + 1;
                let (x, y) = (i * 800_000, j * 800_000);
                pads.push(Pad {
                    shape: Shape::circle(Point::new(x, y), 320_000).unwrap(),
                    ..smd(x, y, net)
                });
                pads.push(smd((i * n + j) * 1_500_000 - 40 * MM, -8 * MM, net));
            }
        }
        let mut d = design(4, pads, (n * n) as usize + 1);
        let mut spec = RuleSpec::new(&["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"], 200_000, 100_000);
        spec.hole_clearance = 160_000;
        spec.hole_to_hole = 250_000;
        d.rules = Rules::new(spec).unwrap();
        for net in &mut d.nets {
            net.track_width = 200_000;
            net.via = Some(ViaSize {
                diameter: 450_000,
                drill: 300_000,
            });
        }
        d
    }

    #[test]
    fn the_balls_deep_in_an_array_that_has_room_for_their_vias_get_them_in_rows() {
        let d = roomy(8);
        let middle = Point::new(2_800_000, 2_800_000);
        // The sixteen balls inside the two rings that get out on their layer.
        let found = balls(&d);
        let inside: Vec<_> = found.iter().filter(|b| b.depth > b.tracks).collect();
        assert_eq!(inside.len(), 16);
        let small = ViaSize {
            diameter: 300_000,
            drill: 200_000,
        };
        for fanout in [Some(small), None] {
            let escapes = plan(&d, &mut world_of(&d), 150_000, fanout, true);
            let (exits, vias): (Vec<_>, Vec<_>) = escapes.iter().partition(|e| e.via.is_none());
            // The second ring's ways out, and a via for each ball further in.
            assert_eq!(exits.len(), 20);
            assert_eq!(vias.len(), 16);
            for e in &vias {
                let via = e.via.as_ref().expect("a via");
                // The smaller one where there is one: a track of 0.2 mm passes between
                // two of those side by side, and not between two of the nets' own.
                let size = if fanout.is_some() {
                    (300_000, 200_000)
                } else {
                    (450_000, 300_000)
                };
                assert_eq!((via.diameter, via.drill), size);
                let [track] = &e.tracks[..] else {
                    panic!("{:?}", e.tracks)
                };
                assert_eq!(track.end, via.at);
                let ball = track.start;
                assert!(inside.iter().any(|b| b.at == ball), "{ball:?}");
                // In the middle of four balls, on the side away from the array's middle.
                let away = |at: i64, from: i64, to: i64| (at - from) * (to - from).signum();
                assert_eq!(away(via.at.x, ball.x, middle.x), -400_000, "{via:?}");
                assert_eq!(away(via.at.y, ball.y, middle.y), -400_000, "{via:?}");
            }
            let mut board = d.board.clone();
            for e in &escapes {
                board.tracks.extend(e.tracks.iter().cloned());
                board.vias.extend(e.via.clone());
            }
            let broken = drc::check(&board, &d.rules).unwrap();
            assert!(broken.is_empty(), "{broken:?}");
        }
        // In a run with no budget there are none, and an array of six by six has too few
        // balls that deep for it: each of those finds its via as it is routed.
        assert!(plan(&d, &mut world_of(&d), 150_000, Some(small), false).is_empty());
        let few = roomy(6);
        let escapes = plan(&few, &mut world_of(&few), 150_000, Some(small), true);
        assert!(escapes.iter().all(|e| e.via.is_none()), "{escapes:?}");
    }
}
