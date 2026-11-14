//! Gates: the passages between fixed copper that are only as wide as a track and its
//! clearances, or less than a pitch of the lattice more.
//!
//! A track fits such a passage in one place, midway, and the lattices that routes move on
//! start at pads' centres: a line of one is in that place by chance. Between the balls of
//! an area array of 0.8 mm pitch, with a track and clearances of 0.15 mm, it never is. So
//! each gate is an *anchor*, a point midway between its two pads or vias, with a lattice of
//! its own that routes may move onto and along near it, as they do near the pads of their
//! own nets. Where a gate between two items side by side meets one between two items one
//! above the other, as between four balls of an array, the anchor is where their two lines
//! cross, so that one lattice goes through both, and a via fits where only the middle of
//! four pads has room for one.
//!
//! Anchors are places to look, and no more: every step onto and along their lattices is
//! checked against the [`World`](super::world::World) as any other.

use super::Design;
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::NetId;
use crate::space::FastMap;

/// How many pitches from its pads an anchor's lattice reaches at most, before the pitch
/// that every reach has on top.
const MAX_REACH: i64 = 16;

/// How many widths of passage are told apart: nets whose tracks and clearances differ have
/// gates of their own.
const CLASSES: usize = 32;

/// A point with a lattice of its own.
#[derive(Clone, Copy, Debug)]
struct Anchor {
    at: Point,
    /// How far from it, in either axis, routes may be on its lattice.
    reach: i64,
    layers: LayerSet,
    /// The widths of passage it is for, as bits by position in [`Gates::needs`].
    classes: u32,
}

/// Which way a gate lets tracks through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    /// Between two items side by side: tracks go up and down, on a line of one `x`.
    Vertical,
    /// Between two items one above the other.
    Horizontal,
    /// Between two items that neither axis lines up.
    Diagonal,
}

/// The anchors of a board, by where they are.
#[derive(Clone, Debug, Default)]
pub(crate) struct Gates {
    anchors: Vec<Anchor>,
    cells: FastMap<(i64, i64), Vec<u32>>,
    cell: i64,
    /// The widths of passage that nets need, a track and its clearances, in order.
    needs: Vec<i64>,
    /// The pitch that a search's lattice has to be finer than for the anchors to be its:
    /// zero for any.
    finer: i64,
}

/// What a passage has to be wide for a track of a net: its width and its clearance on
/// either side, on the first layer.
pub(crate) fn need_of(design: &Design, net: NetId) -> i64 {
    let rules = &design.rules;
    let width = design.nets.get(net.index()).map_or(0, |s| s.track_width);
    width.max(rules.track_width(Some(net), LayerId(0))) + 2 * rules.clearance(Some(net), LayerId(0))
}

impl Gates {
    /// The gates between the pads and vias of a design, for the nets it routes, on a
    /// lattice of `pitch`.
    pub(crate) fn plan(design: &Design, pitch: i64) -> Gates {
        let all = design.rules.layers();
        let mut needs: Vec<i64> = (0..design.nets.len())
            .filter(|&i| design.nets[i].route)
            .map(|i| need_of(design, NetId(i as u32)))
            .collect();
        needs.sort_unstable();
        needs.dedup();
        let mut gates = Gates {
            anchors: Vec::new(),
            cells: FastMap::default(),
            cell: 1,
            needs,
            finer: 0,
        };
        let (Some(&least), Some(&most)) = (gates.needs.first(), gates.needs.last()) else {
            return gates;
        };
        let pitch = pitch.max(1);
        // The copper that passages are between: pads and vias, each with its layers.
        let mut items: Vec<(Shape, LayerSet)> = Vec::new();
        for p in &design.board.pads {
            for (layers, shape) in p.parts() {
                let on = LayerSet(layers.0 & all.0);
                if on.0 != 0 {
                    items.push((shape.clone(), on));
                }
            }
        }
        for v in &design.board.vias {
            if let Ok(s) = Shape::circle(v.at, v.diameter) {
                let on = LayerSet(LayerSet::span(v.from, v.to).0 & all.0);
                if on.0 != 0 {
                    items.push((s, on));
                }
            }
        }
        let index = SpatialIndex::bulk_load(items.iter().map(|(s, _)| s.bbox()));
        let widest = most + pitch;
        let mut near = Vec::new();
        // The gates, each with the pads' boxes it is between.
        let mut found: Vec<(Anchor, Way, Rect)> = Vec::new();
        for (k, (a, on_a)) in items.iter().enumerate() {
            index.query(&a.bbox().expand(widest), &mut near);
            near.sort_unstable_by_key(|id| id.slot());
            for id in &near {
                let m = id.slot() as usize;
                if m <= k {
                    continue;
                }
                let (b, on_b) = &items[m];
                let layers = LayerSet(on_a.0 & on_b.0);
                if layers.0 == 0
                    || a.violates(b, least) != Ok(false)
                    || a.violates(b, widest) != Ok(true)
                {
                    continue;
                }
                // The widths that fit, and that the lattice may miss.
                let mut classes = 0u32;
                for (c, &need) in gates.needs.iter().enumerate() {
                    if a.violates(b, need) == Ok(false) && a.violates(b, need + pitch) == Ok(true) {
                        classes |= 1 << c.min(CLASSES - 1);
                    }
                }
                if classes == 0 {
                    continue;
                }
                let (at, way) = between(&a.bbox(), &b.bbox());
                let around = a.bbox().union(&b.bbox());
                found.push((
                    Anchor {
                        at,
                        reach: reach_of(at, &around, pitch),
                        layers,
                        classes,
                    },
                    way,
                    around,
                ));
            }
        }
        // Where a gate that tracks go up and down through meets one that they go across
        // through, the anchor is where the two lines cross.
        let mut anchors: Vec<Anchor> = Vec::new();
        let mut crossed = vec![false; found.len()];
        let by_place = SpatialIndex::bulk_load(found.iter().map(|(g, ..)| Rect::new(g.at, g.at)));
        for i in 0..found.len() {
            let (g, way, around) = found[i];
            if way != Way::Vertical {
                continue;
            }
            by_place.query(&Rect::new(g.at, g.at).expand(g.reach), &mut near);
            near.sort_unstable_by_key(|id| id.slot());
            for id in &near {
                let j = id.slot() as usize;
                let (h, other, beside) = found[j];
                let (layers, classes) = (LayerSet(g.layers.0 & h.layers.0), g.classes & h.classes);
                if other != Way::Horizontal || layers.0 == 0 || classes == 0 {
                    continue;
                }
                crossed[i] = true;
                crossed[j] = true;
                let at = Point::new(g.at.x, h.at.y);
                anchors.push(Anchor {
                    at,
                    reach: reach_of(at, &around.union(&beside), pitch),
                    layers,
                    classes,
                });
            }
        }
        anchors.extend(
            found
                .iter()
                .zip(&crossed)
                .filter(|(_, c)| !**c)
                .map(|((g, ..), _)| *g),
        );
        // One anchor for each place.
        anchors.sort_unstable_by_key(|a| (a.at, a.layers.0, a.classes, a.reach));
        let mut merged: Vec<Anchor> = Vec::new();
        for a in anchors {
            match merged.last_mut() {
                Some(m) if m.at == a.at => {
                    m.layers = LayerSet(m.layers.0 | a.layers.0);
                    m.classes |= a.classes;
                    m.reach = m.reach.max(a.reach);
                }
                _ => merged.push(a),
            }
        }
        gates.cell = merged.iter().map(|a| 2 * a.reach).max().unwrap_or(1).max(1);
        for (i, a) in merged.iter().enumerate() {
            let cell = gates.cell;
            let (lo, hi) = (
                Point::new(a.at.x - a.reach, a.at.y - a.reach),
                Point::new(a.at.x + a.reach, a.at.y + a.reach),
            );
            for cy in lo.y.div_euclid(cell)..=hi.y.div_euclid(cell) {
                for cx in lo.x.div_euclid(cell)..=hi.x.div_euclid(cell) {
                    gates.cells.entry((cx, cy)).or_default().push(i as u32);
                }
            }
        }
        gates.anchors = merged;
        gates
    }

    /// The same anchors, for the searches on a lattice finer than `pitch` alone.
    pub(crate) fn finer_than(mut self, pitch: i64) -> Gates {
        self.finer = pitch;
        self
    }

    /// Whether there are no anchors at all.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// How many anchors there are.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.anchors.len()
    }

    /// The bit of the anchors for a net that needs a passage of `need`, or zero if no
    /// routed net does.
    pub(crate) fn class(&self, need: i64, pitch: i64) -> u32 {
        if self.finer != 0 && pitch >= self.finer {
            return 0;
        }
        match self.needs.binary_search(&need) {
            Ok(c) => 1 << c.min(CLASSES - 1),
            Err(_) => 0,
        }
    }

    /// The anchors of `classes` whose lattices reach `p` on `layer`, into `out`, in order.
    #[inline]
    pub(crate) fn near(&self, p: Point, layer: u8, classes: u32, out: &mut Vec<Point>) {
        if classes == 0 || self.anchors.is_empty() {
            return;
        }
        let key = (p.x.div_euclid(self.cell), p.y.div_euclid(self.cell));
        let Some(cell) = self.cells.get(&key) else {
            return;
        };
        for &i in cell {
            let a = &self.anchors[i as usize];
            if a.classes & classes != 0
                && a.layers.contains(LayerId(layer))
                && (p.x - a.at.x).abs() <= a.reach
                && (p.y - a.at.y).abs() <= a.reach
            {
                out.push(a.at);
            }
        }
    }
}

/// How far an anchor's lattice reaches: past the far end of the copper it is between, and
/// a pitch more, so that routes can move onto it where there is room to.
fn reach_of(at: Point, around: &Rect, pitch: i64) -> i64 {
    let far = (at.x - around.min().x)
        .max(around.max().x - at.x)
        .max(at.y - around.min().y)
        .max(around.max().y - at.y);
    far.min(MAX_REACH * pitch) + pitch
}

/// The point midway between two boxes, and which way tracks pass between them.
fn between(a: &Rect, b: &Rect) -> (Point, Way) {
    let mid = |lo: i64, hi: i64| lo + (hi - lo) / 2;
    // The part of an axis that both boxes are over, if any, and otherwise the gap.
    let x = (a.min().x.max(b.min().x), a.max().x.min(b.max().x));
    let y = (a.min().y.max(b.min().y), a.max().y.min(b.max().y));
    let way = match (x.0 <= x.1, y.0 <= y.1) {
        (false, true) => Way::Vertical,
        (true, false) => Way::Horizontal,
        _ => Way::Diagonal,
    };
    (Point::new(mid(x.0, x.1), mid(y.0, y.1)), way)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{MM, design, smd};
    use super::Gates;
    use crate::geom::drc::Pad;
    use crate::geom::{Point, Shape};

    /// A grid of balls of 0.35 mm at a pitch of 0.8 mm.
    fn balls(n: i64) -> Vec<Pad> {
        let mut pads = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let (x, y) = (i * 800_000, j * 800_000);
                pads.push(Pad {
                    shape: Shape::circle(Point::new(x, y), 350_000).unwrap(),
                    ..smd(x, y, (i * n + j) as u32 + 1)
                });
            }
        }
        pads
    }

    #[test]
    fn an_array_s_gates_are_anchors_where_their_lines_cross() {
        let mut d = design(2, balls(3), 10);
        // A track of 0.15 mm and its clearances are the 0.45 mm between two balls.
        for n in &mut d.nets {
            n.track_width = 150_000;
        }
        let mut spec = d.rules.spec().clone();
        spec.clearance = 150_000;
        spec.track_width = 150_000;
        d.rules = crate::geom::Rules::new(spec).unwrap();
        let gates = Gates::plan(&d, 150_000);
        // The middles of the four squares of balls, and nothing else.
        assert_eq!(gates.len(), 4);
        let class = gates.class(450_000, 150_000);
        assert_ne!(class, 0);
        let mut near = Vec::new();
        gates.near(Point::new(300_000, 450_000), 0, class, &mut near);
        assert!(near.contains(&Point::new(400_000, 400_000)), "{near:?}");
        // Not on the layer that the balls are not on, nor for a width that no net has.
        near.clear();
        gates.near(Point::new(300_000, 450_000), 1, class, &mut near);
        assert!(near.is_empty());
        assert_eq!(gates.class(460_000, 150_000), 0);
    }

    #[test]
    fn a_passage_with_room_to_spare_is_no_gate() {
        // Pads a millimetre apart: the lattice finds its way between them.
        let d = design(1, vec![smd(0, 0, 1), smd(2 * MM, 0, 2)], 3);
        assert!(Gates::plan(&d, 175_000).is_empty());
        // And pads too close for any track are none either.
        let d = design(1, vec![smd(0, 0, 1), smd(MM, 0, 2)], 3);
        assert!(Gates::plan(&d, 175_000).is_empty());
    }
}
