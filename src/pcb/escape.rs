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

use super::Design;
use super::world::{Verdict, World};
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
    /// The only way out, if no via fits but a track gets out: on a layer, from a pad.
    path: Option<(Vec<Point>, LayerId)>,
    width: i64,
    via: (i64, i64),
    candidates: Vec<Candidate>,
    bbox: Rect,
    done: bool,
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

/// Plans the escapes of a design, against the fixed copper of `world`, into which it puts
/// them as routes of their nets as it goes. `pitch` is detailed routing's lattice.
/// Whether fixed copper of a pad's net already touches it, as routing kept from before does:
/// then it is out already.
fn joined(design: &Design, pad: usize) -> bool {
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

pub(crate) fn plan(design: &Design, world: &mut World<'_>, pitch: i64) -> Vec<Escape> {
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

    let mut regions: Vec<Region> = Vec::new();
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
            let pad = &pads[i];
            let Some((net, spec)) = routed(pad.net) else {
                continue;
            };
            let Some(via) = spec.via else { continue };
            let on = LayerSet(pad.layers.0 & all.0);
            if on == all || region_of.contains_key(&i) || joined(design, i) {
                continue;
            }
            let near_way = flood(world, net, i, design, on, pitch, &out, true, &width_of);
            let Flood::Trapped(reached) = near_way else {
                continue;
            };
            let width = width_of(net, on.iter().next().unwrap_or(LayerId(0)));
            let mut group = vec![i];
            group.extend(reached.iter().copied().filter(|&p| p != i));
            let candidates = candidates(
                world,
                design,
                net,
                &group,
                (via.diameter, via.drill),
                pitch,
                &width_of,
            );
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
                path,
                width,
                via: (via.diameter, via.drill),
                candidates,
                bbox,
                done: false,
            });
        }
    }

    let mut out = Vec::new();
    // Regions with no room for a via get out by the shortest way there is, first, as it is
    // the only one.
    for r in regions.iter_mut().filter(|r| r.candidates.is_empty()) {
        r.done = true;
        let Some((path, layer)) = r.path.take() else {
            continue;
        };
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
                width: r.width,
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
        // The places still free, for each region left.
        for r in regions.iter_mut().filter(|r| !r.done) {
            let (net, (d, drill), width) = (r.net, r.via, r.width);
            r.candidates
                .retain(|c| still_free(world, net, c, d, drill, width));
        }
        let Some(r) = (0..regions.len())
            .filter(|&r| !regions[r].done && !regions[r].candidates.is_empty())
            .min_by_key(|&r| (regions[r].candidates.len(), r))
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
        let mut best: Option<((usize, usize, i128), usize)> = None;
        for (k, c) in regions[r].candidates.iter().enumerate() {
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
            let key = (usize::MAX - least, taken, c.far);
            if best.as_ref().is_none_or(|(b, _)| key < *b) {
                best = Some((key, k));
            }
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
fn candidates(
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
