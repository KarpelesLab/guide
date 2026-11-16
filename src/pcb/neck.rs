//! Neck-down: a net whose tracks are too wide to leave one of its pads is routed narrower.
//!
//! A net's width is its class's, which is a default more than a rule: between the pads of a
//! 0.4 mm QFN, or of a fine-pitch connector, a track of 0.25 mm does not clear the pads on
//! either side, and boards at that pitch are routed narrower there by hand. A pad that no
//! straight track of its net's width leaves, from its centre in any of the eight directions
//! on any of its layers, past the other nets' pads and everything else that is fixed, has
//! its net routed at the widest width that does leave it, in steps of 5 µm, and never under
//! the least width the rules allow. The net has that width everywhere: one width a net.
//!
//! Leaving a pad is not enough where the pads beside it have to be left too: a track of
//! 0.22 mm clears the pads of a 0.4 mm QFN on either side by the 0.19 mm that a board asks
//! for, and two such tracks out of two pads side by side are 0.18 mm apart. Every other
//! pad of the row then gets out, and the rest are shut in by their neighbours' tracks, which
//! no rip-up settles: whichever of two pads is routed shuts the other in. So a pad's track
//! has to leave it *beside* those of the pads next to it: the same way as a track of the
//! same width leaves each neighbour of another net, and clear of it, by the clearance of
//! the rule area that both are in if there is one with its own. In a run with a
//! budget: with none, the narrower tracks of two curated boards found other ways, a
//! connection fewer on one of them.

use super::Design;
use super::detail::DIRS;
use super::router::pad_center;
use super::world::{Verdict, World};
use crate::geom::{LayerId, LayerSet, Point, Shape, SpatialIndex};
use crate::id::NetId;

/// The step in which a narrower width is looked for.
const STEP: i64 = 5_000;

/// A pad beside another: its net, shape, centre and layers.
struct Beside<'a> {
    net: NetId,
    shape: &'a Shape,
    center: Point,
    layers: LayerSet,
}

/// The track that leaves a pad straight in a direction: from `center` to a width past the
/// pad's box.
fn exit(shape: &Shape, center: Point, (dx, dy): (i64, i64), width: i64) -> Option<Shape> {
    let b = shape.bbox();
    // How far the box's edge is in this direction, and a track's width more.
    let reach = |d: i64, lo: i64, hi: i64, at: i64| match d {
        1 => hi - at,
        -1 => at - lo,
        _ => i64::MAX,
    };
    let out =
        reach(dx, b.min().x, b.max().x, center.x).min(reach(dy, b.min().y, b.max().y, center.y))
            + width;
    let to = Point::new(center.x + dx * out, center.y + dy * out);
    Shape::track(center, to, width).ok()
}

/// Whether a track of `net`, `width` wide, leaves the pad straight: from `center` to a
/// width past the pad's box, in one of the eight directions, on one of `layers`. And
/// beside the pads in `beside`: a way that a track of the same width leaves each of them
/// by too, on that layer, clear of this one. A way that only this pad has, as the end of
/// a row has round its corner, is not one that the row's tracks share.
fn leaves(
    world: &World<'_>,
    net: NetId,
    shape: &Shape,
    center: Point,
    layers: LayerSet,
    width: i64,
    beside: &[Beside<'_>],
) -> bool {
    let rules = world.rules();
    DIRS.iter().any(|&dir| {
        let Some(track) = exit(shape, center, dir, width) else {
            return false;
        };
        layers.iter().any(|l| {
            world.track(net, l, &track, None) == Verdict::Free
                && beside.iter().filter(|q| q.layers.contains(l)).all(|q| {
                    exit(q.shape, q.center, dir, width).is_some_and(|other| {
                        // Two tracks in an area with a clearance of its own keep that.
                        let clearance = rules
                            .area_clearance(l, &track, &other)
                            .unwrap_or_else(|| rules.clearance_between(Some(net), Some(q.net), l));
                        track.violates(&other, clearance) != Ok(true)
                            && world.track(q.net, l, &other, None) == Verdict::Free
                    })
                })
        })
    })
}

/// By net, the width to route it at if that is narrower than its own: the widest at which
/// every pad of its that its own width does not leave is left, if there is one, and then,
/// with `beside`, the widest at which it is left beside the pads next to it, if there is
/// one.
pub(crate) fn plan(design: &Design, world: &World<'_>, beside: bool) -> Vec<Option<i64>> {
    let rules = &design.rules;
    let stack = rules.layers();
    let mut out = vec![None; design.nets.len()];
    // By net, its pads with no hole, which are the ones a track has to leave on a layer.
    let mut pads: Vec<Vec<usize>> = vec![Vec::new(); design.nets.len()];
    for (i, p) in design.board.pads.iter().enumerate() {
        if let Some(n) = p.net
            && n.index() < pads.len()
            && p.hole.is_none()
        {
            pads[n.index()].push(i);
        }
    }
    // The pads with no hole of the nets that are routed, to find those beside a pad.
    let routed = |n: NetId| design.nets.get(n.index()).is_some_and(|s| s.route);
    let rows: Vec<usize> = (0..design.board.pads.len())
        .filter(|&p| {
            let pad = &design.board.pads[p];
            pad.hole.is_none() && pad.net.is_some_and(routed) && pad.layers.0 & stack.0 != 0
        })
        .collect();
    let index = SpatialIndex::bulk_load(rows.iter().map(|&p| design.board.pads[p].shape.bbox()));
    // The pads of other nets on a layer of pad `p` with less than a track of `full` and
    // its clearances between them and it.
    let neighbours = |p: usize, full: i64| -> Vec<Beside<'_>> {
        let pad = &design.board.pads[p];
        let clearance = stack
            .iter()
            .map(|l: LayerId| rules.clearance(pad.net, l))
            .max()
            .unwrap_or(0);
        let mut near = Vec::new();
        index.query(&pad.shape.bbox().expand(full + 2 * clearance), &mut near);
        let mut found: Vec<usize> = near.iter().map(|id| rows[id.slot() as usize]).collect();
        found.sort_unstable();
        found
            .into_iter()
            .filter_map(|q| {
                let other = &design.board.pads[q];
                let net = other.net.filter(|&n| Some(n) != pad.net)?;
                let layers = LayerSet(other.layers.0 & pad.layers.0 & stack.0);
                let center = pad_center(&other.shape)?;
                (layers.0 != 0).then_some(Beside {
                    net,
                    shape: &other.shape,
                    center,
                    layers,
                })
            })
            .collect()
    };
    for (i, spec) in design.nets.iter().enumerate() {
        if !spec.route || pads[i].is_empty() {
            continue;
        }
        let net = NetId(i as u32);
        // The least the rules allow on any layer, and the width the net has.
        let least = stack
            .iter()
            .map(|l| rules.track_width(Some(net), l))
            .max()
            .unwrap_or(spec.track_width);
        let full = spec.track_width.max(least);
        if least >= full {
            continue;
        }
        let at = |p: usize| {
            let pad = &design.board.pads[p];
            let layers = LayerSet(pad.layers.0 & stack.0);
            pad_center(&pad.shape).map(|c| (&pad.shape, c, layers))
        };
        // The widest width that gets every pad out that any width gets out: alone, and
        // then beside its neighbours.
        let mut width = full;
        for alone in [true, false] {
            if !alone && !beside {
                break;
            }
            for &p in &pads[i] {
                let Some((shape, c, layers)) = at(p) else {
                    continue;
                };
                let beside = if alone {
                    Vec::new()
                } else {
                    neighbours(p, full)
                };
                if !alone && beside.is_empty() {
                    continue;
                }
                let mut w = width;
                while w > least && !leaves(world, net, shape, c, layers, w, &beside) {
                    w = (w - STEP).max(least);
                }
                if leaves(world, net, shape, c, layers, w, &beside) {
                    width = w;
                }
            }
        }
        if width < full {
            out[i] = Some(width);
        }
    }
    out
}
