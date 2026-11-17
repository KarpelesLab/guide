//! Thermal reliefs kept free: where the spokes of pads in pours will be, as places that
//! other nets' tracks and vias pay to be in.
//!
//! A pad that a zone of its net joins through a thermal relief is joined by its spokes, a
//! few narrow bridges across the gap around it. A track or a via of another net beside
//! the pad takes a spoke away, and a pad left with one, or none, is hard to solder and may
//! be cut off its pour. Nothing in a search for one net says that the room beside another
//! net's pad is wanted: so the room that each spoke needs, from the pad's edge across its
//! gap and on into the pour, is marked on a grid by layer, with the pad's net, and a step
//! of another net's track that ends there costs more for its length. A cost and no wall:
//! where a board has no other way, the route is made and the spoke is lost, as before.
//! Vias pay nothing more: a via beside a pad is reached by a track that pays, and a price
//! on the via itself cost a board connections for no more spokes (`docs/benchmarks.md`).

use super::Design;
use super::zones::{CopperZone, ZoneConnection};
use crate::geom::{LayerId, LayerSet, Point, Rect};
use crate::id::NetId;

/// In the cells of a spoke that is [held](Spokes::plan), the net's index plus one has this
/// bit set.
pub(crate) const HELD: u32 = 1 << 31;

/// How many times what a track costs in the room of a spoke it costs in the room of one
/// that is held.
pub(crate) const HELD_TIMES: f64 = 8.0;

/// Where spokes will be, by cell of a grid and layer.
#[derive(Clone, Debug)]
pub(crate) struct Spokes {
    origin: Point,
    cell: i64,
    cols: usize,
    rows: usize,
    /// By layer and then cell, the net's index plus one where a track of another net would
    /// be in the way of a spoke, or zero.
    tracks: Vec<u32>,
    /// What a track there costs for its length, over its length elsewhere.
    cost: f64,
    /// How many spokes were marked.
    pub(crate) count: usize,
}

/// Whether `p` is inside the ring `ring`, by crossings.
fn inside(ring: &[Point], p: Point) -> bool {
    let mut odd = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let t = (p.y - a.y) as f64 / (b.y - a.y) as f64;
            if (a.x as f64 + t * (b.x - a.x) as f64) > p.x as f64 {
                odd = !odd;
            }
        }
    }
    odd
}

impl Spokes {
    /// The spokes of a design's pads in zones of their nets that join them through thermal
    /// reliefs, on a grid of `cell` over `bounds`, each with the room beside it that a
    /// track of another net is taken to need, `need`: half its width and the clearance.
    /// `None` if there are none.
    pub(crate) fn plan(
        design: &Design,
        bounds: Rect,
        cell: i64,
        need: i64,
        cost: f64,
    ) -> Option<Spokes> {
        let cell = cell.max(1000);
        let layers = design.rules.layer_count();
        let cols = ((bounds.max().x - bounds.min().x) / cell + 1).max(1) as usize;
        let rows = ((bounds.max().y - bounds.min().y) / cell + 1).max(1) as usize;
        let mut out = Spokes {
            origin: bounds.min(),
            cell,
            cols,
            rows,
            tracks: Vec::new(),
            cost,
            count: 0,
        };
        let stack = design.rules.layers();
        // The host's own spokes, if it gave any, are all there are. Those of a pad that
        // has no more of them on a layer than it is held to are held: a track there costs
        // many times what it costs in the room of a spoke that the pad can spare.
        let least = usize::from(design.zones.min_spokes);
        let mut by_pad: std::collections::BTreeMap<(usize, u8), usize> =
            std::collections::BTreeMap::new();
        for spoke in &design.zones.spokes {
            *by_pad.entry((spoke.pad, spoke.layer.0)).or_default() += 1;
        }
        // The held ones first: a cell is of the first spoke that marks it.
        for held in [true, false] {
            for spoke in &design.zones.spokes {
                let net = design.board.pads.get(spoke.pad).and_then(|p| p.net);
                let (Some(net), true) = (net, stack.contains(spoke.layer)) else {
                    continue;
                };
                let few = by_pad
                    .get(&(spoke.pad, spoke.layer.0))
                    .is_some_and(|&n| n <= least);
                if few != held {
                    continue;
                }
                if out.tracks.is_empty() {
                    out.tracks = vec![0; layers * cols * rows];
                }
                out.count += 1;
                out.mark(
                    spoke.layer,
                    net,
                    spoke.from,
                    spoke.to,
                    spoke.width / 2 + need,
                    held,
                );
            }
        }
        let zones: &[CopperZone] = if design.zones.spokes.is_empty() {
            &design.zones.zones
        } else {
            &[]
        };
        for zone in zones {
            let Some(net) = zone.net else { continue };
            if zone.outline.len() < 3 {
                continue;
            }
            let on = LayerSet(zone.layers.0 & stack.0);
            for (i, pad) in design.board.pads.iter().enumerate() {
                if pad.net != Some(net) {
                    continue;
                }
                let shared = LayerSet(pad.copper_layers().0 & on.0);
                if shared.0 == 0 {
                    continue;
                }
                let own = design.zones.pads.get(i).copied().unwrap_or_default();
                let thermal = match own.connection.unwrap_or(zone.connection) {
                    ZoneConnection::Thermal => true,
                    ZoneConnection::ThermalThroughHole => pad.hole.is_some(),
                    _ => false,
                };
                let b = pad.shape.bbox();
                let anchor = own.anchor.unwrap_or_else(|| {
                    Point::new(
                        b.min().x + (b.max().x - b.min().x) / 2,
                        b.min().y + (b.max().y - b.min().y) / 2,
                    )
                });
                if !thermal
                    || !inside(&zone.outline, anchor)
                    || zone.holes.iter().any(|h| inside(h, anchor))
                {
                    continue;
                }
                for (a, e, half) in spokes_of(zone, own, b, anchor, cell) {
                    if out.tracks.is_empty() {
                        out.tracks = vec![0; layers * cols * rows];
                    }
                    out.count += 1;
                    for l in shared.iter() {
                        out.mark(l, net, a, e, half + need, false);
                    }
                }
            }
        }
        (out.count > 0).then_some(out)
    }

    /// Marks the cells of a layer whose centres are within `reach` of the segment from `a`
    /// to `b`, where no other net has marked them.
    fn mark(&mut self, layer: LayerId, net: NetId, a: Point, b: Point, reach: i64, held: bool) {
        let lo = (a.x.min(b.x) - reach, a.y.min(b.y) - reach);
        let hi = (a.x.max(b.x) + reach, a.y.max(b.y) + reach);
        let col =
            |x: i64| ((x - self.origin.x).div_euclid(self.cell)).clamp(0, self.cols as i64 - 1);
        let row =
            |y: i64| ((y - self.origin.y).div_euclid(self.cell)).clamp(0, self.rows as i64 - 1);
        let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
        let len2 = dx * dx + dy * dy;
        let reach2 = (reach as f64) * (reach as f64);
        for r in row(lo.1)..=row(hi.1) {
            for c in col(lo.0)..=col(hi.0) {
                let x = (self.origin.x + c * self.cell + self.cell / 2 - a.x) as f64;
                let y = (self.origin.y + r * self.cell + self.cell / 2 - a.y) as f64;
                let t = if len2 > 0.0 {
                    ((x * dx + y * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (ex, ey) = (x - t * dx, y - t * dy);
                if ex * ex + ey * ey > reach2 {
                    continue;
                }
                let at = r as usize * self.cols + c as usize;
                let slot = &mut self.tracks[usize::from(layer.0) * self.cols * self.rows + at];
                if *slot == 0 {
                    *slot = (net.0 + 1) | if held { HELD } else { 0 };
                }
            }
        }
    }

    fn at(&self, p: Point) -> Option<usize> {
        let (x, y) = (p.x - self.origin.x, p.y - self.origin.y);
        if x < 0 || y < 0 {
            return None;
        }
        let (c, r) = ((x / self.cell) as usize, (y / self.cell) as usize);
        (c < self.cols && r < self.rows).then(|| r * self.cols + c)
    }

    /// The net whose pad's spoke a track on `layer` ending at `p` would be in the way of,
    /// as its index plus one, or zero, and what that costs over the track's length.
    pub(crate) fn toll(&self, layer: u8, p: Point) -> (u32, f64) {
        let n = self
            .at(p)
            .and_then(|at| {
                self.tracks
                    .get(usize::from(layer) * self.cols * self.rows + at)
            })
            .copied()
            .unwrap_or(0);
        (n, self.cost)
    }

    /// What a track of `net` on `layer` ending at `p` costs for its length: more where a
    /// spoke of another net's pad will be.
    #[inline]
    pub(crate) fn factor(&self, net: NetId, layer: u8, p: Point) -> f64 {
        match self.at(p).and_then(|at| {
            self.tracks
                .get(usize::from(layer) * self.cols * self.rows + at)
        }) {
            Some(&n) if n != 0 && n & !HELD != net.0 + 1 => {
                1.0 + self.cost * if n & HELD != 0 { HELD_TIMES } else { 1.0 }
            }
            _ => 1.0,
        }
    }
}

/// The spokes of a pad in `zone`: each from the pad's edge to where it is in the pour, and
/// half its width. As the fill builds them, [`fill_zones`](super::fill_zones).
fn spokes_of(
    zone: &CopperZone,
    own: super::PadConnection,
    b: Rect,
    anchor: Point,
    past: i64,
) -> Vec<(Point, Point, i64)> {
    let (dx, dy) = own
        .spoke_direction
        .or(zone.spoke_direction)
        .unwrap_or((1, 0));
    let norm = ((dx as f64) * (dx as f64) + (dy as f64) * (dy as f64)).sqrt();
    if norm == 0.0 {
        return Vec::new();
    }
    let (ux, uy) = (dx as f64 / norm, dy as f64 / norm);
    let half = 0.5f64;
    let turn = match zone.spokes {
        1 => Some((1, (1.0, 0.0))),
        2 => Some((2, (-1.0, 0.0))),
        3 => Some((3, (-half, 0.75f64.sqrt()))),
        6 => Some((6, (half, 0.75f64.sqrt()))),
        8 => Some((8, (half.sqrt(), half.sqrt()))),
        _ => None,
    };
    let dirs: Vec<(f64, f64)> = match turn {
        None => vec![(ux, uy), (-uy, ux), (-ux, -uy), (uy, -ux)],
        Some((count, (c, s))) => {
            let mut dirs = vec![(ux, uy)];
            for k in 1..count {
                let (vx, vy): (f64, f64) = dirs[k - 1];
                dirs.push((vx * c - vy * s, vx * s + vy * c));
            }
            dirs
        }
    };
    let corners = [
        (b.min().x, b.min().y),
        (b.max().x, b.min().y),
        (b.max().x, b.max().y),
        (b.min().x, b.max().y),
    ];
    let reach: Vec<f64> = match own.spoke_reach.filter(|_| matches!(dirs.len(), 2 | 4)) {
        Some((a, _)) if dirs.len() == 2 => vec![a as f64, a as f64],
        Some((a, b)) => vec![a as f64, b as f64, a as f64, b as f64],
        None => dirs
            .iter()
            .map(|&(vx, vy)| {
                corners
                    .iter()
                    .map(|&(x, y)| (x - anchor.x) as f64 * vx + (y - anchor.y) as f64 * vy)
                    .fold(0.0, f64::max)
            })
            .collect(),
    };
    let narrowest = reach.iter().copied().fold(f64::MAX, f64::min);
    let width = (own.spoke_width.unwrap_or(zone.spoke_width) as f64).min(2.0 * narrowest);
    if width < zone.min_width as f64 || width <= 0.0 {
        return Vec::new();
    }
    let gap = own.thermal_gap.unwrap_or(zone.thermal_gap) as f64;
    dirs.iter()
        .zip(&reach)
        .map(|(&(vx, vy), &extent)| {
            let at = |along: f64| {
                Point::new(
                    anchor.x + (vx * along).round() as i64,
                    anchor.y + (vy * along).round() as i64,
                )
            };
            // Across the gap and a little into the pour, which the spoke has to reach.
            let end = extent + gap + (zone.min_width / 2) as f64 + past as f64;
            (at(extent), at(end), (width / 2.0) as i64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::tests::{MM, design, smd};
    use super::super::{CopperZone, ZoneConnection};
    use super::Spokes;
    use crate::geom::{LayerSet, Point, Rect};
    use crate::id::NetId;

    #[test]
    fn the_spokes_of_a_pad_with_none_to_spare_are_held() {
        // Two pads of net 1 with spokes that the host gives: the first has two, the
        // second four, and a pad is to keep two.
        let mut d = design(
            2,
            vec![
                smd(5 * MM, 5 * MM, 1),
                smd(15 * MM, 5 * MM, 1),
                smd(8 * MM, 8 * MM, 2),
            ],
            3,
        );
        let spoke = |pad: usize, x: i64, dx: i64, dy: i64| super::super::Spoke {
            pad,
            layer: crate::geom::LayerId(0),
            from: Point::new(x + dx * 300_000, 5 * MM + dy * 300_000),
            to: Point::new(x + dx * 900_000, 5 * MM + dy * 900_000),
            width: 300_000,
        };
        d.zones.spokes = vec![
            spoke(0, 5 * MM, 1, 0),
            spoke(0, 5 * MM, -1, 0),
            spoke(1, 15 * MM, 1, 0),
            spoke(1, 15 * MM, -1, 0),
            spoke(1, 15 * MM, 0, 1),
            spoke(1, 15 * MM, 0, -1),
        ];
        let bounds = Rect::new(Point::new(-MM, -MM), Point::new(21 * MM, 11 * MM));
        let plan =
            |d: &super::super::Design| Spokes::plan(d, bounds, 100_000, 300_000, 4.0).unwrap();
        let (few, many) = (
            Point::new(5_600_000, 5 * MM),
            Point::new(15_600_000, 5 * MM),
        );
        // With no number to keep, every spoke costs the same.
        let s = plan(&d);
        assert_eq!(s.count, 6);
        assert_eq!(
            (s.factor(NetId(2), 0, few), s.factor(NetId(2), 0, many)),
            (5.0, 5.0)
        );
        // Held to two, the pad with two has none to spare.
        d.zones.min_spokes = 2;
        let s = plan(&d);
        assert_eq!(
            (s.factor(NetId(2), 0, few), s.factor(NetId(2), 0, many)),
            (33.0, 5.0)
        );
        assert_eq!(s.factor(NetId(1), 0, few), 1.0);
    }

    #[test]
    fn the_room_of_a_pad_s_spokes_is_marked_for_other_nets() {
        // A pad of net 1, 1 mm square, in a zone of its net on the first layer, with a gap
        // of 0.3 mm and spokes of 0.3 mm along the axes.
        let mut d = design(2, vec![smd(5 * MM, 5 * MM, 1), smd(8 * MM, 8 * MM, 2)], 3);
        let ring = vec![
            Point::new(0, 0),
            Point::new(10 * MM, 0),
            Point::new(10 * MM, 10 * MM),
            Point::new(0, 10 * MM),
        ];
        d.zones.zones.push(CopperZone {
            outline: ring,
            layers: LayerSet::EMPTY.with(crate::geom::LayerId(0)),
            net: Some(NetId(1)),
            connection: ZoneConnection::Thermal,
            thermal_gap: 300_000,
            spoke_width: 300_000,
            min_width: 200_000,
            ..CopperZone::default()
        });
        let bounds = Rect::new(Point::new(-MM, -MM), Point::new(11 * MM, 11 * MM));
        let s = Spokes::plan(&d, bounds, 100_000, 300_000, 4.0).unwrap();
        assert_eq!(s.count, 4);
        let half = d.board.pads[0].shape.bbox().max().x - 5 * MM;
        // In the gap to the right of the pad, on the spoke's axis.
        let in_gap = Point::new(5 * MM + half + 150_000, 5 * MM);
        assert_eq!(s.factor(NetId(2), 0, in_gap), 5.0);
        assert_eq!(s.factor(NetId(1), 0, in_gap), 1.0);
        assert_eq!(s.factor(NetId(2), 1, in_gap), 1.0);
        // Off the corner of the pad, between two spokes, there is room.
        let corner = Point::new(5 * MM + half + 600_000, 5 * MM + half + 600_000);
        assert_eq!(s.factor(NetId(2), 0, corner), 1.0);
        // A zone that joins its pads solidly has no spokes to keep.
        d.zones.zones[0].connection = ZoneConnection::Solid;
        assert!(Spokes::plan(&d, bounds, 100_000, 300_000, 4.0).is_none());
        // Spokes that the host gives are those that count, whatever the zones say: one
        // along a diagonal, on the second layer.
        d.zones.spokes.push(super::super::Spoke {
            pad: 0,
            layer: crate::geom::LayerId(1),
            from: Point::new(5 * MM, 5 * MM),
            to: Point::new(6 * MM, 6 * MM),
            width: 200_000,
        });
        let s = Spokes::plan(&d, bounds, 100_000, 300_000, 4.0).unwrap();
        assert_eq!(s.count, 1);
        let on_it = Point::new(5 * MM + 700_000, 5 * MM + 700_000);
        assert_eq!(s.factor(NetId(2), 1, on_it), 5.0);
        assert_eq!(s.factor(NetId(2), 0, on_it), 1.0);
        assert_eq!(s.factor(NetId(1), 1, on_it), 1.0);
        // And nothing where the zone's own would have been, on the pad's other side.
        assert_eq!(s.factor(NetId(2), 0, Point::new(4 * MM, 5 * MM)), 1.0);
        assert_eq!(s.factor(NetId(2), 1, Point::new(4 * MM, 5 * MM)), 1.0);
    }
}
