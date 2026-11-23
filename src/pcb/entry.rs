//! Pad entry: a track is no wider than a pad of its net where it meets it.
//!
//! A net's width is its class's, and a pad of the net can be narrower: a pin of a QFN
//! 0.2 mm wide on a supply routed at 0.3 mm. A track wider than the pad covers its end and
//! its sides, the pad is then one piece of copper with the track, and at soldering it has
//! the track's heat sink on its whole outline where a pad means to have a neck. A board is
//! laid out by hand with the track narrowed for its last stretch, and [`neck_at_pads`]
//! does that to tracks that are routed already: every part of a straight track that is
//! nearer to a narrower pad of its net than the track's own half width and a margin is cut
//! out of the track and laid again at the pad's width, along the same line. What is laid
//! is inside the copper that was there, so nothing is nearer to another net than it was,
//! and every end is where it was, so nothing that was joined comes apart.
//!
//! The width of a pad is the host's to say ([`PadEntry::width`]): a host that judges a pad
//! by a measure of its own gives that, never under the least width its rules allow.
//! [`pad_entries`] has the plain one, the shorter side of the pad's box.

use super::Design;
use crate::geom::drc::Track;
use crate::geom::{LayerSet, Point, Shape};
use crate::id::NetId;

/// The step along a track at which its distance to a pad is first looked at; where it
/// changes sides between two steps, the place is then found to [`FINE`] of a step.
const STEP: i64 = 5_000;

/// The parts a step is cut in.
const FINE: i64 = 1 << 10;

/// A pad as [`neck_at_pads`] takes it: copper of a net on some layers, and the widest that
/// a track of the net may be where it meets it.
#[derive(Clone, Debug)]
pub struct PadEntry<'a> {
    /// The copper.
    pub shape: &'a Shape,
    /// The layers it is on.
    pub layers: LayerSet,
    /// Its net.
    pub net: NetId,
    /// The widest a track may be at the pad: the pad's own width, and never less than the
    /// least width the rules allow a track.
    pub width: i64,
}

/// The pads of a design with a net as [`neck_at_pads`] takes them, each with the shorter
/// side of its box for a width, or the least width that the rules give its net on any
/// layer if that is more. A pad stack's other shapes are entries of their own.
pub fn pad_entries(design: &Design) -> Vec<PadEntry<'_>> {
    let rules = &design.rules;
    let mut out = Vec::new();
    for pad in &design.board.pads {
        let Some(net) = pad.net else { continue };
        let shapes = core::iter::once((pad.layers, &pad.shape))
            .chain(pad.stack.iter().map(|(layers, shape)| (*layers, shape)));
        for (layers, shape) in shapes {
            let least = layers
                .iter()
                .map(|l| rules.track_width(Some(net), l))
                .max()
                .unwrap_or(0);
            let b = shape.bbox();
            let side = (b.max().x - b.min().x).min(b.max().y - b.min().y);
            out.push(PadEntry {
                shape,
                layers,
                net,
                width: side.max(least),
            });
        }
    }
    out
}

/// The point of a track `k` parts of `of` from its start.
fn along(t: &Track, k: i64, of: i64) -> Point {
    let on = |a: i64, b: i64| a + ((i128::from(b - a) * i128::from(k)) / i128::from(of)) as i64;
    Point::new(on(t.start.x, t.end.x), on(t.start.y, t.end.y))
}

/// Whether `at` is nearer to `shape` than `reach`.
fn near(shape: &Shape, at: Point, reach: i64) -> bool {
    // A disc of radius 1 for the point: its gap to the shape is the point's less 1.
    Shape::circle(at, 2).is_ok_and(|dot| shape.violates(&dot, reach - 1) != Ok(false))
}

/// The tracks with every part that is near a narrower pad of its net laid at that pad's
/// [width](PadEntry::width): each track of `tracks` in order, whole or in the pieces it is
/// cut in from its start to its end, with its position in `tracks`.
///
/// A part of a track is near a pad when its middle line is closer to the pad's copper than
/// half the track's width and `beyond`: what stays at the track's own width then has
/// `beyond` between its copper and the pad's. A host that reads a pad's joints in a band
/// round its outline gives that band's width and a little more. Arc tracks are left as
/// they are, and so is a track on no layer of the pad.
pub fn neck_at_pads(pads: &[PadEntry<'_>], tracks: &[Track], beyond: i64) -> Vec<(usize, Track)> {
    let beyond = beyond.max(0);
    // The pads by net, in order.
    let mut by_net: Vec<(NetId, usize)> =
        pads.iter().enumerate().map(|(i, p)| (p.net, i)).collect();
    by_net.sort_unstable();
    let mut out = Vec::with_capacity(tracks.len());
    for (i, t) in tracks.iter().enumerate() {
        let cuts = match t.net {
            Some(net) if t.mid.is_none() && t.start != t.end => {
                let from = by_net.partition_point(|&(n, _)| n < net);
                let own = by_net[from..]
                    .iter()
                    .take_while(|&&(n, _)| n == net)
                    .map(|&(_, p)| &pads[p]);
                narrow_parts(t, own, beyond)
            }
            _ => Vec::new(),
        };
        if cuts.is_empty() {
            out.push((i, t.clone()));
            continue;
        }
        // The track from cut to cut, each piece at the least width of the parts over it.
        let of = cuts[0].2;
        let mut marks: Vec<i64> = cuts.iter().flat_map(|c| [c.0, c.1]).collect();
        marks.extend([0, of]);
        marks.sort_unstable();
        marks.dedup();
        let mut pieces: Vec<(i64, i64, i64)> = Vec::new();
        for w in marks.windows(2) {
            let width = cuts
                .iter()
                .filter(|c| c.0 <= w[0] && w[1] <= c.1)
                .map(|c| c.3)
                .min()
                .unwrap_or(t.width);
            match pieces.last_mut() {
                Some(last) if last.2 == width => last.1 = w[1],
                _ => pieces.push((w[0], w[1], width)),
            }
        }
        for (a, b, width) in pieces {
            let (start, end) = (along(t, a, of), along(t, b, of));
            if start != end {
                out.push((
                    i,
                    Track {
                        start,
                        end,
                        width,
                        ..t.clone()
                    },
                ));
            }
        }
    }
    out
}

/// The parts of a track near a narrower pad of `own`: from and to, in parts of the third
/// value from the track's start, and the width to lay the part at.
fn narrow_parts<'a>(
    t: &Track,
    own: impl Iterator<Item = &'a PadEntry<'a>>,
    beyond: i64,
) -> Vec<(i64, i64, i64, i64)> {
    let reach = t.width / 2 + beyond;
    let (dx, dy) = ((t.end.x - t.start.x) as f64, (t.end.y - t.start.y) as f64);
    let steps = ((dx.hypot(dy) / STEP as f64).ceil() as i64).max(1);
    let of = steps * FINE;
    let track_box = crate::geom::Rect::new(t.start, t.end);
    let mut out = Vec::new();
    for pad in own {
        if pad.width >= t.width || !pad.layers.contains(t.layer) {
            continue;
        }
        let window = pad.shape.bbox().expand(reach + STEP);
        if !window.intersects(&track_box) {
            continue;
        }
        // The steps of the track inside the pad's window.
        let inside = |k: i64| {
            let q = along(t, k, steps);
            window.intersects(&crate::geom::Rect::new(q, q))
        };
        let span = |from: i64, to: i64, lo: i64, hi: i64| -> (i64, i64) {
            // Along one axis the track is in the window between two of its steps.
            if from == to {
                return if lo <= from && from <= hi {
                    (0, steps)
                } else {
                    (1, 0)
                };
            }
            let at = |v: i64| (v - from) as f64 / (to - from) as f64 * steps as f64;
            let (a, b) = (at(lo), at(hi));
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            (
                (a.floor() as i64 - 1).max(0),
                (b.ceil() as i64 + 1).min(steps),
            )
        };
        let (x0, x1) = span(t.start.x, t.end.x, window.min().x, window.max().x);
        let (y0, y1) = span(t.start.y, t.end.y, window.min().y, window.max().y);
        let (first, last) = (x0.max(y0), x1.min(y1));
        if first > last {
            continue;
        }
        let is_near = |k: i64| near(pad.shape, along(t, k, of), reach);
        // Between a fine place that is not near and one that is, the last that is not.
        let edge = |mut off: i64, mut on: i64| {
            while (on - off).abs() > 1 {
                let mid = off + (on - off) / 2;
                if is_near(mid) {
                    on = mid;
                } else {
                    off = mid;
                }
            }
            off
        };
        let mut k = first;
        while k <= last {
            if !(inside(k) && is_near(k * FINE)) {
                k += 1;
                continue;
            }
            let begun = k;
            while k < last && is_near((k + 1) * FINE) {
                k += 1;
            }
            let from = if begun == 0 {
                0
            } else if is_near((begun - 1) * FINE) {
                // Near before the window too, which a window wider than the reach rules out
                // but for a pad whose box is not its copper's.
                (begun - 1) * FINE
            } else {
                edge((begun - 1) * FINE, begun * FINE)
            };
            let to = if k == steps {
                of
            } else {
                edge((k + 1) * FINE, k * FINE)
            };
            if from < to {
                out.push((from, to, of, pad.width));
            }
            k += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{LayerId, Rect};

    fn track(a: (i64, i64), b: (i64, i64), width: i64) -> Track {
        Track {
            start: Point::new(a.0, a.1),
            end: Point::new(b.0, b.1),
            mid: None,
            width,
            layer: LayerId(0),
            net: Some(NetId(1)),
        }
    }

    fn pad(shape: &Shape, width: i64) -> PadEntry<'_> {
        PadEntry {
            shape,
            layers: LayerSet::EMPTY.with(LayerId(0)),
            net: NetId(1),
            width,
        }
    }

    /// A track 0.3 mm wide that ends in a pad 0.2 mm wide is 0.2 mm wide from the pad's
    /// middle to where its own copper would be clear of the pad, and 0.3 mm from there on;
    /// the ends are where they were.
    #[test]
    fn a_wide_track_is_narrow_where_it_meets_a_narrow_pad() {
        let shape = Shape::rect(Rect::new(
            Point::new(-400_000, -100_000),
            Point::new(400_000, 100_000),
        ))
        .unwrap();
        let pads = [pad(&shape, 200_000)];
        let t = track((0, 0), (0, 2_000_000), 300_000);
        let out = neck_at_pads(&pads, core::slice::from_ref(&t), 15_000);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.iter().all(|(i, _)| *i == 0));
        let (narrow, wide) = (&out[0].1, &out[1].1);
        assert_eq!((narrow.start, narrow.width), (t.start, 200_000));
        assert_eq!((wide.end, wide.width), (t.end, 300_000));
        assert_eq!(narrow.end, wide.start);
        // The pad's edge is at 0.1 mm; half the width and the margin beyond it.
        let cut = narrow.end.y;
        assert!((265_000..=266_000).contains(&cut), "{cut}");
        // The wide piece keeps the margin from the pad.
        let rest = Shape::track(wide.start, wide.end, wide.width).unwrap();
        assert_eq!(shape.violates(&rest, 15_000), Ok(false));
        assert_eq!(shape.violates(&rest, 16_500), Ok(true));
    }

    /// A track no wider than the pad, one of another net, one on another layer and an arc
    /// come back as they are; a track that passes a narrow pad of its net is narrow beside
    /// it and wide before and after.
    #[test]
    fn only_what_is_near_a_narrower_pad_of_the_net_is_cut() {
        let shape = Shape::circle(Point::new(0, 0), 200_000).unwrap();
        let pads = [pad(&shape, 150_000)];
        let same = track((0, 0), (1_000_000, 0), 150_000);
        let mut other = track((0, 0), (1_000_000, 0), 300_000);
        other.net = Some(NetId(2));
        let mut below = track((0, 0), (1_000_000, 0), 300_000);
        below.layer = LayerId(1);
        let mut arc = track((0, 0), (1_000_000, 0), 300_000);
        arc.mid = Some(Point::new(500_000, 100_000));
        let far = track((0, 1_000_000), (1_000_000, 1_000_000), 300_000);
        let given = [same, other, below, arc, far];
        let out = neck_at_pads(&pads, &given, 15_000);
        assert_eq!(out.len(), given.len());
        for (k, (i, t)) in out.iter().enumerate() {
            assert_eq!((*i, t), (k, &given[k]));
        }
        let past = track((-1_000_000, 50_000), (1_000_000, 50_000), 300_000);
        let out = neck_at_pads(&pads, core::slice::from_ref(&past), 15_000);
        let widths: Vec<i64> = out.iter().map(|(_, t)| t.width).collect();
        assert_eq!(widths, [300_000, 150_000, 300_000]);
        assert_eq!((out[0].1.start, out[2].1.end), (past.start, past.end));
        assert_eq!(
            (out[0].1.end, out[1].1.end),
            (out[1].1.start, out[2].1.start)
        );
        for wide in [&out[0].1, &out[2].1] {
            let rest = Shape::track(wide.start, wide.end, wide.width).unwrap();
            assert_eq!(shape.violates(&rest, 15_000), Ok(false));
        }
    }

    /// Two narrow pads on one track: each part at its pad's width, the part between them
    /// at the track's if there is room for one, and a track wholly near a pad narrow from
    /// end to end.
    #[test]
    fn a_track_between_two_narrow_pads() {
        let a = Shape::circle(Point::new(0, 0), 200_000).unwrap();
        let b = Shape::circle(Point::new(2_000_000, 0), 250_000).unwrap();
        let pads = [pad(&a, 160_000), pad(&b, 200_000)];
        let t = track((0, 0), (2_000_000, 0), 300_000);
        let out = neck_at_pads(&pads, &[t], 15_000);
        let widths: Vec<i64> = out.iter().map(|(_, t)| t.width).collect();
        assert_eq!(widths, [160_000, 300_000, 200_000]);
        let short = track((0, 0), (150_000, 0), 300_000);
        let out = neck_at_pads(&pads, core::slice::from_ref(&short), 15_000);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].1,
            Track {
                width: 160_000,
                ..short
            }
        );
    }

    /// The pads of a design: the shorter side of each, and the rules' least width where
    /// a pad is narrower than that.
    #[test]
    fn the_entries_of_a_design() {
        use crate::geom::drc::{Board, Pad};
        use crate::geom::{RuleSpec, Rules};
        let rules = Rules::new(RuleSpec::new(&["F.Cu", "B.Cu"], 150_000, 150_000)).unwrap();
        let top = LayerSet::EMPTY.with(LayerId(0));
        let rect =
            |w: i64, h: i64| Shape::rect(Rect::new(Point::new(0, 0), Point::new(w, h))).unwrap();
        let mut board = Board::default();
        board
            .pads
            .push(Pad::new(rect(200_000, 900_000), top, Some(NetId(0))));
        board
            .pads
            .push(Pad::new(rect(100_000, 900_000), top, Some(NetId(0))));
        board.pads.push(Pad::new(rect(500_000, 500_000), top, None));
        let design = Design {
            board,
            rules,
            outline: Vec::new(),
            edge_clearance: 0,
            nets: vec![super::super::NetSpec::new(250_000, None)],
            obstacles: Vec::new(),
            parts: Vec::new(),
            pairs: Vec::new(),
            lengths: Vec::new(),
            zones: Default::default(),
        };
        let widths: Vec<i64> = pad_entries(&design).iter().map(|e| e.width).collect();
        assert_eq!(widths, [200_000, 150_000]);
    }
}
