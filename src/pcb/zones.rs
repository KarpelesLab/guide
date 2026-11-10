//! Copper zones: outlines filled with copper of their net, around everything else.
//!
//! A zone's fill is its outline, on each of its layers, less what other nets' copper needs
//! around it: every pad, track, via, graphic and text of another net grown by the clearance,
//! every hole of another net grown by the hole clearance, the board's edge grown by the edge
//! clearance, solder mask openings of other nets, keepouts of copper pours, and the fills of
//! zones of other nets with a higher priority. Pads of its own net join it as each says:
//! solidly, through a thermal relief of four spokes across a gap, or not at all. What is
//! narrower than the zone's minimum width is taken away, and so are islands that nothing of
//! the net touches, unless the zone keeps them.
//!
//! This is what KiCad's `ZONE_FILLER` does, and `docs/benchmarks.md` compares the two on the
//! curated boards. The geometry is exact but for arcs, which are within a micrometre of the
//! true ones, on the side of the obstacle, and for the rounding of new vertices, which a
//! margin of a few nanometres over every clearance makes up for, so that a fill never comes
//! closer to anything than the rules let it.

use core::cmp::Reverse;

use crate::geom::drc::{self, Board};
use crate::geom::{ArcSide, Area, GeomError, LayerId, LayerSet, Point, Rect, Shape, SpatialIndex};
use crate::id::NetId;

use super::Design;

/// How a zone joins the pads of its net.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoneConnection {
    /// Through a thermal relief: a gap around the pad, bridged by spokes.
    #[default]
    Thermal,
    /// Solidly: the fill covers the pad.
    Solid,
    /// Not at all: the fill keeps its clearance from the pad, as from another net's.
    None,
    /// Through a thermal relief for pads with a hole, and solidly for the others.
    ThermalThroughHole,
}

/// What a zone does with pieces of its fill that no copper of its net touches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Islands {
    /// Takes them away.
    #[default]
    Remove,
    /// Keeps them.
    Keep,
    /// Takes away those smaller than this many square nanometres.
    RemoveBelow(i128),
}

/// A copper zone: an outline to fill on some layers, with copper of its net.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopperZone {
    /// The outline, a ring in either orientation.
    pub outline: Vec<Point>,
    /// Holes in the outline, which are not filled.
    pub holes: Vec<Vec<Point>>,
    /// The layers it is filled on.
    pub layers: LayerSet,
    /// Its net, or `None` for copper of no net.
    pub net: Option<NetId>,
    /// Zones of a higher priority are filled first, and those of other nets are kept clear
    /// of by those of lower priority.
    pub priority: u32,
    /// The clearance it keeps from other nets' copper, at least: the rules' clearance
    /// applies if it is larger.
    pub clearance: i64,
    /// The narrowest a part of its fill may be.
    pub min_width: i64,
    /// How it joins its net's pads, unless a pad says otherwise.
    pub connection: ZoneConnection,
    /// The gap around a pad of a thermal relief.
    pub thermal_gap: i64,
    /// The width of a thermal relief's spokes.
    pub spoke_width: i64,
    /// What it does with islands.
    pub islands: Islands,
}

/// What one pad says of how zones join it, in place of the zones' own settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadConnection {
    /// How zones join it.
    pub connection: Option<ZoneConnection>,
    /// The gap of its thermal relief.
    pub thermal_gap: Option<i64>,
    /// The width of its thermal relief's spokes.
    pub spoke_width: Option<i64>,
    /// Where its spokes start, if not the centre of its box.
    pub anchor: Option<Point>,
    /// The direction of its first spoke, as a vector of any length, the others turning by
    /// right angles from it. `None` is along the X axis.
    pub spoke_direction: Option<(i64, i64)>,
    /// How far the pad reaches from its anchor along its first and third spokes, and along
    /// its second and fourth, if not as far as its box reaches.
    pub spoke_reach: Option<(i64, i64)>,
}

/// A design's copper zones: their outlines, what pads say of them, and where they may not
/// be filled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zones {
    /// The zones.
    pub zones: Vec<CopperZone>,
    /// By position in [`Board::pads`], what each pad says. Pads past the end say nothing.
    pub pads: Vec<PadConnection>,
    /// Areas that no zone is filled into, on their layers.
    pub keepouts: Vec<(Shape, LayerSet)>,
}

/// A piece of a zone's fill: connected copper on one layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    /// The zone, by position in [`Zones::zones`].
    pub zone: usize,
    /// The layer.
    pub layer: LayerId,
    /// The copper, a polygon with holes.
    pub shape: Shape,
}

impl Fill {
    /// The fill as a zone of the [`drc`] model, of its zone's net.
    pub fn to_zone(&self, design: &Design) -> drc::Zone {
        drc::Zone {
            shape: self.shape.clone(),
            layer: self.layer,
            net: design.zones.zones.get(self.zone).and_then(|z| z.net),
            clearance: None,
        }
    }
}

/// How closely arcs of grown shapes follow the true ones: a micrometre.
const TOLERANCE: i64 = 1000;

/// What every clearance is widened by, for the rounding of new vertices, which moves edges
/// by less than a nanometre.
const MARGIN: i64 = 5;

/// How far a spoke reaches past its pad's gap, besides half the zone's minimum width.
const SPOKE_EPSILON: i64 = 1000;

/// How far copper keeps from a solder mask opening of another net beyond its margin, as in
/// routing.
const MASK_GUARD: i64 = 1000;

/// What a piece of copper is, for filling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Pad(usize),
    Other,
}

struct Copper {
    shape: Shape,
    layers: LayerSet,
    net: Option<NetId>,
    own: Option<i64>,
    kind: Kind,
}

struct Hole {
    shape: Shape,
    net: Option<NetId>,
    own: Option<i64>,
}

fn same_net(a: Option<NetId>, b: Option<NetId>) -> bool {
    a.is_some() && a == b
}

/// Fills a design's [zones](Design::zones) around the copper of `board`, which is the
/// design's board with any routes added, such as [`Routed::board`](super::Routed::board)
/// gives without its fills. The board's own zones are taken as fixed copper, such as filled
/// graphics, and not as fills.
///
/// Zones are filled from the highest priority down, and each on each of its layers, so the
/// result is in that order: by priority, then position, then layer, then piece.
pub fn fill_zones(board: &Board, design: &Design) -> Result<Vec<Fill>, GeomError> {
    Ok(fill_all(board, design)?.0)
}

/// The thermal reliefs that the zones' fills, filled as [`fill_zones`] fills them, join to
/// their pads by a single spoke: each as the pad, by position in [`Board::pads`], and the
/// layer. KiCad's DRC reports them as `starved_thermal`, with its default of two spokes at
/// least. Reliefs with no spoke at all are left out: their pads reach the fill some other
/// way, or not at all.
pub(crate) fn starved_reliefs(
    board: &Board,
    design: &Design,
) -> Result<Vec<(usize, LayerId)>, GeomError> {
    Ok(fill_all(board, design)?.1)
}

/// The fills, and the reliefs of a single spoke.
#[allow(clippy::type_complexity)]
fn fill_all(
    board: &Board,
    design: &Design,
) -> Result<(Vec<Fill>, Vec<(usize, LayerId)>), GeomError> {
    let zones = &design.zones.zones;
    if zones.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let rules = &design.rules;
    let stack = rules.layers();
    let mut copper: Vec<Copper> = Vec::new();
    let mut holes: Vec<Hole> = Vec::new();
    for t in &board.tracks {
        let shape = match t.mid {
            None => Shape::track(t.start, t.end, t.width)?,
            Some(m) => Shape::arc_track(t.start, m, t.end, t.width, board.arc_tolerance)?,
        };
        copper.push(Copper {
            shape,
            layers: LayerSet::EMPTY.with(t.layer),
            net: t.net,
            own: None,
            kind: Kind::Other,
        });
    }
    for v in &board.vias {
        copper.push(Copper {
            shape: Shape::circle(v.at, v.diameter)?,
            layers: LayerSet::span(v.from, v.to),
            net: v.net,
            own: None,
            kind: Kind::Other,
        });
        holes.push(Hole {
            shape: Shape::circle(v.at, v.drill)?,
            net: v.net,
            own: None,
        });
    }
    for (i, p) in board.pads.iter().enumerate() {
        copper.push(Copper {
            shape: p.shape.clone(),
            layers: p.layers,
            net: p.net,
            own: p.clearance,
            kind: Kind::Pad(i),
        });
        if let Some(h) = &p.hole {
            holes.push(Hole {
                shape: h.clone(),
                net: p.net,
                own: p.clearance,
            });
        }
    }
    for z in &board.zones {
        copper.push(Copper {
            shape: z.shape.clone(),
            layers: LayerSet::EMPTY.with(z.layer),
            net: z.net,
            own: z.clearance,
            kind: Kind::Other,
        });
    }
    for (shape, layer) in &design.obstacles {
        copper.push(Copper {
            shape: shape.clone(),
            layers: LayerSet::EMPTY.with(*layer),
            net: None,
            own: None,
            kind: Kind::Other,
        });
    }
    let by_layer: Vec<(Vec<usize>, SpatialIndex)> = (0..rules.layer_count())
        .map(|l| {
            let on: Vec<usize> = (0..copper.len())
                .filter(|&i| copper[i].layers.contains(LayerId(l as u8)))
                .collect();
            let index = SpatialIndex::bulk_load(on.iter().map(|&i| copper[i].shape.bbox()));
            (on, index)
        })
        .collect();
    let hole_index = SpatialIndex::bulk_load(holes.iter().map(|h| h.shape.bbox()));

    let mut order: Vec<usize> = (0..zones.len()).collect();
    order.sort_by_key(|&i| (Reverse(zones[i].priority), i));
    let mut done: Vec<(usize, LayerId, Area)> = Vec::new();
    let mut out = Vec::new();
    let ctx = Ctx {
        design,
        board,
        copper: &copper,
        holes: &holes,
        by_layer: &by_layer,
        hole_index: &hole_index,
        starved: core::cell::RefCell::new(Vec::new()),
    };
    for z in order {
        let zone = &zones[z];
        let outline = outline_area(zone)?;
        if outline.is_empty() {
            continue;
        }
        for layer in LayerSet(zone.layers.0 & stack.0).iter() {
            let fill = ctx.fill(z, layer, &outline, &done)?;
            for shape in fill.shapes() {
                out.push(Fill {
                    zone: z,
                    layer,
                    shape,
                });
            }
            done.push((z, layer, fill));
        }
    }
    Ok((out, ctx.starved.into_inner()))
}

/// A zone's outline less its holes.
fn outline_area(zone: &CopperZone) -> Result<Area, GeomError> {
    let outline = Area::from_rings(&[&zone.outline])?;
    if zone.holes.is_empty() {
        return Ok(outline);
    }
    let holes: Vec<&[Point]> = zone.holes.iter().map(Vec::as_slice).collect();
    outline.minus_all(&[Area::from_rings(&holes)?])
}

struct Ctx<'a> {
    design: &'a Design,
    board: &'a Board,
    copper: &'a [Copper],
    holes: &'a [Hole],
    by_layer: &'a [(Vec<usize>, SpatialIndex)],
    hole_index: &'a SpatialIndex,
    /// Reliefs with a single spoke, as each layer of each zone is filled.
    starved: core::cell::RefCell<Vec<(usize, LayerId)>>,
}

/// A thermal relief to build: the pad, its gap and its spokes' width.
struct Relief {
    pad: usize,
    gap: i64,
    width: i64,
}

impl Ctx<'_> {
    /// How a zone joins a pad of its net: the connection, the gap and the spokes' width.
    fn connection(&self, zone: &CopperZone, pad: usize) -> (ZoneConnection, i64, i64) {
        let own = self.design.zones.pads.get(pad).copied().unwrap_or_default();
        let mode = match own.connection.unwrap_or(zone.connection) {
            ZoneConnection::ThermalThroughHole => {
                if self.board.pads[pad].hole.is_some() {
                    ZoneConnection::Thermal
                } else {
                    ZoneConnection::Solid
                }
            }
            m => m,
        };
        (
            mode,
            own.thermal_gap.unwrap_or(zone.thermal_gap),
            own.spoke_width.unwrap_or(zone.spoke_width),
        )
    }

    fn fill(
        &self,
        z: usize,
        layer: LayerId,
        outline: &Area,
        done: &[(usize, LayerId, Area)],
    ) -> Result<Area, GeomError> {
        let design = self.design;
        let rules = &design.rules;
        let zone = &design.zones.zones[z];
        let Some(bbox) = outline.bbox() else {
            return Ok(Area::empty());
        };
        let grow = |shape: &Shape, by: i64| Area::of_shape(shape, by, TOLERANCE, ArcSide::Outside);
        // What other nets need: taken away from the fill and from its spokes.
        let mut others: Vec<Area> = Vec::new();
        // What the net's own pads need: gaps of thermal reliefs and clearances.
        let mut own: Vec<Area> = Vec::new();
        let mut reliefs: Vec<Relief> = Vec::new();
        let spec = rules.spec();
        let reach = rules
            .max_clearance()
            .max(zone.clearance)
            .max(zone.thermal_gap)
            .max(spec.hole_clearance)
            .max(design.edge_clearance)
            + self.copper.iter().filter_map(|c| c.own).fold(0, i64::max)
            + MARGIN;
        let near_box = bbox.expand(reach);
        for e in &design.outline {
            if e.bbox().intersects(&near_box) {
                // From the edge's line itself, as KiCad does, whatever its width.
                let clearance = design.edge_clearance.max(0);
                others.push(Area::of_centerline(e, clearance + MARGIN, TOLERANCE)?);
            }
        }
        for (shape, layers) in &design.zones.keepouts {
            if layers.contains(layer) && shape.bbox().intersects(&bbox) {
                others.push(grow(shape, 0)?);
            }
        }
        for m in &design.board.masks {
            if m.layer == layer
                && !same_net(zone.net, m.net)
                && m.shape.bbox().intersects(&near_box)
            {
                others.push(grow(&m.shape, m.margin.max(0) + MASK_GUARD + MARGIN)?);
            }
        }
        let (on, index) = &self.by_layer[usize::from(layer.0)];
        let mut near = Vec::new();
        index.query(&near_box, &mut near);
        for id in &near {
            let c = &self.copper[on[id.slot() as usize]];
            if same_net(zone.net, c.net) {
                let Kind::Pad(p) = c.kind else { continue };
                let (mode, gap, width) = self.connection(zone, p);
                match mode {
                    ZoneConnection::Solid | ZoneConnection::ThermalThroughHole => {}
                    ZoneConnection::None => {
                        let clearance =
                            drc::clearance(rules, (zone.net, None), (c.net, c.own), layer)
                                .max(zone.clearance);
                        own.push(grow(&c.shape, clearance + MARGIN)?);
                    }
                    ZoneConnection::Thermal => {
                        own.push(grow(&c.shape, gap.max(0))?);
                        if width > 0 {
                            reliefs.push(Relief { pad: p, gap, width });
                        }
                    }
                }
            } else {
                let clearance = drc::clearance(rules, (zone.net, None), (c.net, c.own), layer)
                    .max(zone.clearance);
                others.push(grow(&c.shape, clearance + MARGIN)?);
            }
        }
        self.hole_index.query(&near_box, &mut near);
        for id in &near {
            let h = &self.holes[id.slot() as usize];
            if same_net(zone.net, h.net) {
                continue;
            }
            let floor = spec.hole_clearance;
            let clearance = h.own.map_or(floor, |o| o.max(floor));
            others.push(grow(&h.shape, clearance + MARGIN)?);
        }
        for (other, l, area) in done {
            let theirs = &design.zones.zones[*other];
            if *l != layer || same_net(zone.net, theirs.net) || area.is_empty() {
                continue;
            }
            let clearance = rules
                .clearance_between(zone.net, theirs.net, layer)
                .max(zone.clearance)
                .max(theirs.clearance);
            for shape in area.shapes() {
                if shape.bbox().intersects(&near_box) {
                    others.push(grow(&shape, clearance + MARGIN)?);
                }
            }
        }
        let allowed = outline.minus_all(&others)?;
        let fill = allowed.minus_all(&own)?;
        let mut fill = fill.opening(zone.min_width / 2, TOLERANCE)?;

        // Thermal reliefs: spokes from each pad across its gap, kept where their end is in
        // the fill, or in another spoke, as KiCad keeps them, and then kept clear of other
        // nets.
        if !reliefs.is_empty() && !fill.is_empty() {
            let mut all = Vec::new();
            let mut owner = Vec::new();
            for (k, r) in reliefs.iter().enumerate() {
                let made = self.spokes(r, zone.min_width)?;
                owner.extend(core::iter::repeat_n(k, made.len()));
                all.extend(made);
            }
            let mut count = vec![0usize; reliefs.len()];
            let mut spokes = Vec::new();
            for (i, (spoke, end)) in all.iter().enumerate() {
                let reaches = fill.contains_point(*end)
                    || all
                        .iter()
                        .enumerate()
                        .any(|(j, (other, _))| j != i && other.contains_point(*end));
                if reaches {
                    let spoke = spoke.and(&allowed)?;
                    if !spoke.is_empty() {
                        spokes.push(spoke);
                        count[owner[i]] += 1;
                    }
                }
            }
            let mut starved = self.starved.borrow_mut();
            for (r, &n) in reliefs.iter().zip(&count) {
                if n == 1 {
                    starved.push((r.pad, layer));
                }
            }
            if !spokes.is_empty() {
                spokes.push(fill);
                fill = Area::union_all(&spokes)?;
            }
        }

        // Islands: pieces that no copper of the net touches.
        if zone.islands == Islands::Keep {
            return Ok(fill);
        }
        let mut kept = Vec::new();
        for piece in fill.pieces() {
            let below = match zone.islands {
                Islands::RemoveBelow(min) => piece.area2() < 2 * min,
                _ => true,
            };
            if !below || self.touches_net(zone, layer, &piece) {
                kept.push(piece);
            }
        }
        Area::union_all(&kept)
    }

    /// Whether copper of the zone's net, which the fill doesn't keep clear of, touches a
    /// piece of fill.
    fn touches_net(&self, zone: &CopperZone, layer: LayerId, piece: &Area) -> bool {
        let Some(bbox) = piece.bbox() else {
            return false;
        };
        let shapes = piece.shapes();
        let (on, index) = &self.by_layer[usize::from(layer.0)];
        let mut near = Vec::new();
        index.query(&bbox, &mut near);
        near.iter().any(|id| {
            let c = &self.copper[on[id.slot() as usize]];
            if !same_net(zone.net, c.net) {
                return false;
            }
            if let Kind::Pad(p) = c.kind
                && self.connection(zone, p).0 == ZoneConnection::None
            {
                return false;
            }
            shapes.iter().any(|s| s.touches(&c.shape))
        })
    }

    /// The four spokes of a pad's thermal relief, each from just behind the pad's anchor
    /// out past its gap, with the point at its end that must be in the fill for it to be
    /// kept: KiCad's `buildThermalSpokes`.
    fn spokes(&self, r: &Relief, min_width: i64) -> Result<Vec<(Area, Point)>, GeomError> {
        let pad = &self.board.pads[r.pad];
        let own = self
            .design
            .zones
            .pads
            .get(r.pad)
            .copied()
            .unwrap_or_default();
        let b: Rect = pad.shape.bbox();
        let anchor = own.anchor.unwrap_or_else(|| {
            Point::new(
                b.min().x + (b.max().x - b.min().x) / 2,
                b.min().y + (b.max().y - b.min().y) / 2,
            )
        });
        let (dx, dy) = own.spoke_direction.unwrap_or((1, 0));
        let norm = ((dx as f64) * (dx as f64) + (dy as f64) * (dy as f64)).sqrt();
        if norm == 0.0 {
            return Ok(Vec::new());
        }
        let (ux, uy) = (dx as f64 / norm, dy as f64 / norm);
        let corners = [
            (b.min().x, b.min().y),
            (b.max().x, b.min().y),
            (b.max().x, b.max().y),
            (b.min().x, b.max().y),
        ];
        let dirs = [(ux, uy), (-uy, ux), (-ux, -uy), (uy, -ux)];
        // How far the pad reaches along each spoke: as it says, or from its box.
        let reach: Vec<f64> = match own.spoke_reach {
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
        // No wider than the pad, and no narrower than the zone's minimum width.
        let narrowest = reach.iter().copied().fold(f64::MAX, f64::min);
        let width = (r.width as f64).min(2.0 * narrowest);
        if width < min_width as f64 || width <= 0.0 {
            return Ok(Vec::new());
        }
        let half = (width / 2.0).floor();
        let mut out = Vec::with_capacity(4);
        for (&(vx, vy), &extent) in dirs.iter().zip(&reach) {
            let length = extent + r.gap as f64 + (min_width / 2) as f64 + SPOKE_EPSILON as f64;
            let (nx, ny) = (-vy * half, vx * half);
            let at = |along: f64, side: f64| {
                Point::new(
                    anchor.x + (vx * along + nx * side).round() as i64,
                    anchor.y + (vy * along + ny * side).round() as i64,
                )
            };
            let ring = [
                at(-half, -1.0),
                at(length, -1.0),
                at(length, 1.0),
                at(-half, 1.0),
            ];
            out.push((Area::from_rings(&[&ring])?, at(length, 1.0)));
        }
        Ok(out)
    }
}
