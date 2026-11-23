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
//! the rule area that both are in if there is one with its own. And no wider than the
//! pitch of the row has room for two of by the board's clearance ([`row_width`]): the
//! tracks of a row leave the area side by side. In a run with a
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

/// What a track's middle is short of in the room between two pads of a row when it is
/// looked at on a grid: a track that leaves a pad a pitch from the next is on the points of
/// a grid no finer than a quarter of the pitch along a diagonal, and passes nearer what is
/// between two of them than the points say. cadlab's router keeps as much (`fanout::necks`).
fn sampling(pitch: f64) -> f64 {
    let s = pitch * core::f64::consts::SQRT_2 / 4.0;
    (pitch * pitch + s * s / 4.0).sqrt() - pitch
}

/// What the rules' distances are rounded by where a width is taken from them.
const SLACK: i64 = 5_000;

/// The widest that a track of `net` may be out of a pad of a row, for the track out of
/// the pad next to it to be as wide and both to go on side by side: the pitch of the two
/// pads less the net's clearance, the board's own and not an area's. In
/// the courtyard of a QFN that has a clearance of its own, two tracks out of two pads
/// side by side are legal nearer than that, and are the board's clearance apart from where
/// they leave it: 80 tracks that have to part at the courtyard's edge are a ring that
/// nothing crosses, and a net 0.22 mm wide beside a clearance of 0.2 mm does not pass
/// between two pins of a header that a net 0.185 mm wide passes between
/// (`tinytapeout-demo`: 300 vias and 5376 mm over four orders at the class's width, 270
/// and 4967 mm at this one, which is the width cadlab's router routes the same nets at;
/// `docs/benchmarks.md`). Only for a pad on one layer with no room for a track of `width`
/// between it and the nearest pad of another net beside it; `width` where that does not
/// hold, and never under `least`.
#[allow(clippy::too_many_arguments)]
fn row_width(
    rules: &crate::geom::Rules,
    net: NetId,
    shape: &Shape,
    center: Point,
    layers: LayerSet,
    width: i64,
    least: i64,
    beside: &[Beside<'_>],
) -> i64 {
    let mut on = layers.iter();
    let (Some(l), None) = (on.next(), on.next()) else {
        return width;
    };
    let far =
        |q: &Beside<'_>| ((q.center.x - center.x) as f64).hypot((q.center.y - center.y) as f64);
    let Some(next) = beside
        .iter()
        .filter(|q| q.layers.contains(l))
        .min_by(|a, b| far(a).total_cmp(&far(b)))
    else {
        return width;
    };
    let pitch = far(next);
    let b = shape.bbox();
    let lat = (b.max().x - b.min().x).min(b.max().y - b.min().y);
    let clearance = rules.clearance(Some(net), l);
    let margin = sampling(pitch);
    let between = pitch - (lat as f64) < (width + 2 * clearance) as f64;
    let crowded = (width + clearance + SLACK) as f64 + margin > pitch;
    if !(between && crowded) {
        return width;
    }
    let cap = pitch - (clearance + SLACK) as f64 - margin;
    (((cap / STEP as f64).floor() as i64) * STEP).clamp(least, width)
}

/// By net, the width to route it at if that is narrower than its own: the widest at which
/// every pad of its that its own width does not leave is left, if there is one, and then,
/// with `beside`, the widest at which it is left beside the pads next to it, if there is
/// one, and no wider than a row of pads at its pitch has room for ([`row_width`]). And the
/// tightest pitch of the tracks of the nets that a row made narrower, half their width and
/// their clearance, if a row made any narrower: the nets of a package are many, and a
/// lattice that their tracks do not fit side by side on wastes the room between them, where
/// a net that is narrower for one pad of its own is one net.
pub(crate) fn plan(
    design: &Design,
    world: &World<'_>,
    beside: bool,
) -> (Vec<Option<i64>>, Option<i64>) {
    let rules = &design.rules;
    let stack = rules.layers();
    let mut out = vec![None; design.nets.len()];
    let mut row_pitch: Option<i64> = None;
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
        let mut in_row = false;
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
                // Beside its neighbours, no wider than what a row at its pitch has room
                // for where the board's own clearance holds.
                let mut w = if alone {
                    width
                } else {
                    width.min(row_width(
                        rules, net, shape, c, layers, width, least, &beside,
                    ))
                };
                // At a row's width the pad has only to be left: the tracks beside it are
                // as narrow.
                let row = w < width;
                let beside = if row { Vec::new() } else { beside };
                while w > least && !leaves(world, net, shape, c, layers, w, &beside) {
                    w = (w - STEP).max(least);
                }
                if leaves(world, net, shape, c, layers, w, &beside) {
                    in_row |= row && w < width;
                    width = w;
                }
            }
        }
        if width < full {
            out[i] = Some(width);
            if in_row {
                let pitch = ((width + rules.clearance(Some(net), LayerId(0))) / 2).max(1000);
                row_pitch = Some(row_pitch.map_or(pitch, |p| p.min(pitch)));
            }
        }
    }
    (out, row_pitch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rect, RuleSpec, Rules};

    /// A pad 0.2 mm or `wide` across and 0.8 mm long at the origin, and the one beside it
    /// `pitch` away, of another net.
    fn row(wide: i64, pitch: i64) -> (Shape, Shape) {
        let pad = |x: i64| {
            Shape::rect(Rect::new(
                Point::new(x - wide / 2, -400_000),
                Point::new(x + wide / 2, 400_000),
            ))
            .unwrap()
        };
        (pad(0), pad(pitch))
    }

    fn width_of(clearance: i64, wide: i64, pitch: i64, width: i64, layers: LayerSet) -> i64 {
        let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 100_000, clearance)).unwrap();
        let (own, next) = row(wide, pitch);
        let beside = [Beside {
            net: NetId(1),
            shape: &next,
            center: Point::new(pitch, 0),
            layers: LayerSet::first(1),
        }];
        row_width(
            &rules,
            NetId(0),
            &own,
            Point::new(0, 0),
            layers,
            width,
            160_000,
            &beside,
        )
    }

    #[test]
    fn a_track_out_of_a_row_is_no_wider_than_the_pitch_has_room_for_two_of() {
        let top = LayerSet::first(1);
        // A QFN of 0.4 mm with a clearance of 0.2 mm: 0.185 mm, where cadlab's router
        // has the same net.
        assert_eq!(width_of(200_000, 200_000, 400_000, 220_000, top), 185_000);
        // A connector of 0.5 mm with pads 0.3 mm wide, a supply 0.25 mm wide with a
        // clearance of 0.25 mm: 0.235 mm.
        assert_eq!(width_of(250_000, 300_000, 500_000, 250_000, top), 235_000);
        // And with a clearance of 0.2 mm two tracks of 0.22 mm have the room: as it was.
        assert_eq!(width_of(200_000, 300_000, 500_000, 220_000, top), 220_000);
        // Never under the least width: a pitch of 0.4 mm less 0.25 mm is 0.14 mm.
        assert_eq!(width_of(250_000, 200_000, 400_000, 250_000, top), 160_000);
        // A pad with room for a track between it and the next is no pad of a row.
        assert_eq!(width_of(200_000, 200_000, 900_000, 220_000, top), 220_000);
        // Nor is a pad on two layers, which a track may leave on the other.
        assert_eq!(
            width_of(200_000, 200_000, 400_000, 220_000, LayerSet::first(2)),
            220_000
        );
    }
}
