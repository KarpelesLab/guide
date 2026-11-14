//! The only file that may name polyclip.
//!
//! cargo treats every polyclip 0.0.x release as breaking, so a polyclip type in `guide`'s
//! public API would force `guide` and cadlab to upgrade in lockstep. Keeping every use in
//! one private module makes that hard to do by accident, in three ways:
//!
//! - nothing here is more visible than `pub(crate)`, and the `private_interfaces` lint,
//!   denied for the whole geometry module, rejects a public signature that uses it;
//! - there are plain functions in place of `From` impls, since a trait impl between public
//!   types would be public whichever module it lived in;
//! - `polyclip_stays_private` in the tests fails if any other file under `src/geom` names
//!   polyclip in code, or if this one has an item plainly `pub` or a trait impl.

use polyclip::{
    ArcTol, Boolean, Circle, Curve, EndCap, FillRule, Geometry, Join, Op, Polygon, Ring, Side,
    offset, offset_paths, opening,
};

use super::types::{GeomError, Point};

fn to_poly(p: Point) -> polyclip::Point {
    polyclip::Point::new(p.x, p.y)
}

fn from_poly(p: polyclip::Point) -> Point {
    Point::new(p.x, p.y)
}

/// The part of a shape that polyclip measures, with no width. A shape is every point within
/// some radius of its core.
///
/// Every point in here has been range-checked. That matters more than it looks, because
/// polyclip's queries answer `false` for input out of range, and `false` from
/// `distance_less_than` would mean "no violation".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Core {
    /// A circle's centre.
    Point(polyclip::Point),
    /// A track's centerline.
    Segment(polyclip::Segment),
    /// An approximated arc's centerline.
    Path(polyclip::Path),
    /// A polygon with holes, whose interior belongs to it.
    Polygon(Polygon),
}

impl Core {
    pub(crate) fn point(p: Point) -> Core {
        Core::Point(to_poly(p))
    }

    pub(crate) fn segment(a: Point, b: Point) -> Core {
        Core::Segment(polyclip::Segment {
            a: to_poly(a),
            b: to_poly(b),
        })
    }

    /// A polygon, checked to be simple and properly nested. polyclip's distance queries
    /// assume that, and a self-crossing outline has no clear inside to measure from anyway.
    pub(crate) fn polygon(outer: &[Point], holes: &[&[Point]]) -> Result<Core, GeomError> {
        let ring = |points: &[Point]| Ring(points.iter().copied().map(to_poly).collect());
        let polygon = Polygon::new(ring(outer), holes.iter().map(|h| ring(h)).collect());
        polyclip::validate(&polygon).map_err(|e| GeomError::InvalidPolygon {
            reason: e.to_string(),
        })?;
        Ok(Core::Polygon(polygon))
    }

    /// The polygons, with their holes, that rings enclose under the non-zero winding rule:
    /// one for each connected piece. The rings may have any orientation, and touch,
    /// overlap or cross themselves and each other.
    ///
    /// polyclip's union rounds each new crossing to the nearest integer point, moving
    /// edges through it by at most `sqrt(2)/2`. Rings that only touch along shared edges,
    /// as KiCad's fractured fills do, have no new crossings, and come out exact.
    pub(crate) fn region(rings: &[&[Point]]) -> Result<Vec<Core>, GeomError> {
        let rings: Vec<Ring> = rings
            .iter()
            .map(|r| Ring(r.iter().copied().map(to_poly).collect()))
            .collect();
        let set = polyclip::union_all(&rings, FillRule::NonZero).map_err(|e| {
            GeomError::InvalidPolygon {
                reason: e.to_string(),
            }
        })?;
        Ok(set.into_iter().map(Core::Polygon).collect())
    }

    /// The centerline of the arc from `start` through `mid` to `end`, as a polyline.
    ///
    /// polyclip only approximates arcs as parts of closed contours, so this closes the arc
    /// with a chord, tags the edges, and takes the chord off again. The polyline may run
    /// from `end` to `start`, which distances don't care about. `tolerance` is the most by
    /// which the exact polyline and the arc may be apart, on either side, before polyclip
    /// rounds its vertices to integers.
    pub(crate) fn arc(
        start: Point,
        mid: Point,
        end: Point,
        tolerance: i64,
    ) -> Result<Core, GeomError> {
        const ARC: u64 = 1;
        const CHORD: u64 = 2;
        let mut contour = vec![Curve::Arc {
            mid: to_poly(mid),
            end: to_poly(end),
        }];
        if start != end {
            contour.push(Curve::Line(to_poly(start)));
        }
        let tol = ArcTol::new(tolerance, Side::Nearest);
        let rings = polyclip::Shape::new(contour, Vec::new())
            .to_tagged(tol, &|_, element| if element == 0 { ARC } else { CHORD })
            .map_err(|e| GeomError::InvalidArc {
                reason: e.to_string(),
            })?;
        let Some(ring) = rings.first() else {
            return Ok(Core::segment(start, end));
        };
        let n = ring.points.len();
        // Walk the ring from the end of the chord, if there is one, so as to meet the
        // arc's edges in one run.
        let first = (0..n).find(|&i| ring.tags[i] == CHORD).map_or(0, |i| i + 1);
        let mut points = Vec::with_capacity(n + 1);
        for k in 0..n {
            let i = (first + k) % n;
            if ring.tags[i] == ARC {
                if points.is_empty() {
                    points.push(ring.points[i]);
                }
                points.push(ring.points[(i + 1) % n]);
            }
        }
        // Collinear points leave no arc edges, or none that aren't the chord itself.
        if points.len() < 2 {
            return Ok(Core::segment(start, end));
        }
        Ok(Core::Path(polyclip::Path(points)))
    }

    /// The vertices of a segment or a polyline, in order. `None` for points and polygons.
    #[cfg_attr(not(feature = "dsn"), allow(dead_code))]
    pub(crate) fn polyline(&self) -> Option<Vec<Point>> {
        match self {
            Core::Segment(s) => Some(vec![from_poly(s.a), from_poly(s.b)]),
            Core::Path(path) => Some(path.0.iter().copied().map(from_poly).collect()),
            Core::Point(_) | Core::Polygon(_) => None,
        }
    }

    /// A polygon's rings, the outline first and then its holes. `None` for other cores.
    pub(crate) fn rings(&self) -> Option<Vec<Vec<Point>>> {
        match self {
            Core::Polygon(p) => Some(polygon_rings(p)),
            _ => None,
        }
    }

    /// Whether this is a polygon, which has an inside.
    pub(crate) fn is_polygon(&self) -> bool {
        matches!(self, Core::Polygon(_))
    }

    /// Calls `f` with each edge, stopping when it returns `true`, and returns whether it
    /// did. A point is one edge of length zero, so that every core has an edge.
    pub(crate) fn any_edge(&self, mut f: impl FnMut(Point, Point) -> bool) -> bool {
        match self {
            Core::Point(p) => f(from_poly(*p), from_poly(*p)),
            Core::Segment(s) => f(from_poly(s.a), from_poly(s.b)),
            Core::Path(path) => path
                .0
                .windows(2)
                .any(|w| f(from_poly(w[0]), from_poly(w[1]))),
            Core::Polygon(polygon) => polygon
                .rings()
                .any(|ring| ring.edges().any(|(a, b)| f(from_poly(a), from_poly(b)))),
        }
    }

    /// A polyline's edges, each as a segment, or `None` for any other core.
    pub(crate) fn path_segments(&self) -> Option<Vec<Core>> {
        match self {
            Core::Path(path) => Some(
                path.0
                    .windows(2)
                    .map(|w| Core::Segment(polyclip::Segment { a: w[0], b: w[1] }))
                    .collect(),
            ),
            _ => None,
        }
    }

    /// The corners of the bounding box of the vertices.
    pub(crate) fn bounds(&self) -> (Point, Point) {
        // Constructors never make an empty core, so the fallback is never used.
        self.geometry()
            .bbox()
            .map_or((Point::new(0, 0), Point::new(0, 0)), |r| {
                (from_poly(r.min), from_poly(r.max))
            })
    }

    fn geometry(&self) -> &dyn Geometry {
        match self {
            Core::Point(p) => p,
            Core::Segment(s) => s,
            Core::Path(p) => p,
            Core::Polygon(p) => p,
        }
    }

    /// Whether the exact distance between the cores is strictly below `d`.
    pub(crate) fn closer_than(&self, other: &Core, d: i64) -> bool {
        polyclip::distance_less_than(self.geometry(), other.geometry(), d)
    }

    /// Whether the cores share a point, exactly.
    pub(crate) fn intersects(&self, other: &Core) -> bool {
        polyclip::intersects(self.geometry(), other.geometry())
    }
}

/// An area of copper, such as a zone's fill, as polygons with holes: what zone filling adds,
/// takes away and measures. Every operation is polyclip's, exact but for the rounding of new
/// crossings and of the vertices of arcs and offsets, which moves no edge by more than a
/// unit, besides the tolerance of arcs.
///
/// Its polygons are in polyclip's canonical form, so that equal areas compare equal and every
/// result is the same on every platform.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Area {
    polygons: Vec<Polygon>,
}

/// The side that arcs are approximated on, as an area grows or shrinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArcSide {
    /// The approximation holds the true area: for obstacles.
    Outside,
    /// The true area holds the approximation.
    Inside,
}

fn arc_tol(tolerance: i64, side: ArcSide) -> ArcTol {
    let side = match side {
        ArcSide::Outside => Side::Outside,
        ArcSide::Inside => Side::Inside,
    };
    ArcTol::new(tolerance.max(1), side)
}

/// A polygon's rings, the outline first and then its holes.
fn polygon_rings(p: &Polygon) -> Vec<Vec<Point>> {
    core::iter::once(&p.outer)
        .chain(&p.holes)
        .map(|r| r.0.iter().copied().map(from_poly).collect())
        .collect()
}

fn geometry_error(e: polyclip::Error) -> GeomError {
    GeomError::InvalidPolygon {
        reason: e.to_string(),
    }
}

impl Area {
    /// Nothing.
    pub(crate) fn empty() -> Area {
        Area::default()
    }

    /// What rings enclose under the non-zero winding rule, in any orientation.
    pub(crate) fn from_rings(rings: &[&[Point]]) -> Result<Area, GeomError> {
        let rings: Vec<Ring> = rings
            .iter()
            .map(|r| Ring(r.iter().copied().map(to_poly).collect()))
            .collect();
        let polygons = polyclip::union_all(&rings, FillRule::NonZero).map_err(geometry_error)?;
        Ok(Area { polygons })
    }

    /// A shape grown by `by`, which may be negative for a polygon, with arcs, of circles,
    /// tracks' ends and grown corners, within `tolerance` of the true ones, on `side`.
    pub(crate) fn of_shape(
        shape: &super::shape::Shape,
        by: i64,
        tolerance: i64,
        side: ArcSide,
    ) -> Result<Area, GeomError> {
        // Half the doubled radius and error, rounded away from the true shape.
        let base = match side {
            ArcSide::Outside => (shape.radius2 + shape.error2 + 1) / 2,
            ArcSide::Inside => shape.radius2 / 2,
        };
        Area::of_core(shape, base + by, tolerance, side)
    }

    /// A shape's core, its centreline for a track, grown by `by`, as
    /// [`of_shape`](Area::of_shape) grows the whole shape.
    pub(crate) fn of_centerline(
        shape: &super::shape::Shape,
        by: i64,
        tolerance: i64,
    ) -> Result<Area, GeomError> {
        Area::of_core(
            shape,
            by + (shape.error2 + 1) / 2,
            tolerance,
            ArcSide::Outside,
        )
    }

    fn of_core(
        shape: &super::shape::Shape,
        r: i64,
        tolerance: i64,
        side: ArcSide,
    ) -> Result<Area, GeomError> {
        let tol = arc_tol(tolerance, side);
        let polygons = match &shape.core {
            Core::Point(p) => {
                if r <= 0 {
                    Vec::new()
                } else {
                    let ring = Circle::new(*p, r).to_ring(tol).map_err(geometry_error)?;
                    vec![Polygon::new(ring, Vec::new())]
                }
            }
            Core::Segment(s) => {
                if r <= 0 {
                    Vec::new()
                } else if s.a == s.b {
                    let ring = Circle::new(s.a, r).to_ring(tol).map_err(geometry_error)?;
                    vec![Polygon::new(ring, Vec::new())]
                } else {
                    let path = polyclip::Path(vec![s.a, s.b]);
                    offset_paths(&path, r, Join::Round, EndCap::Round, tol)
                        .map_err(geometry_error)?
                }
            }
            Core::Path(path) => {
                if r <= 0 {
                    Vec::new()
                } else {
                    offset_paths(path, r, Join::Round, EndCap::Round, tol)
                        .map_err(geometry_error)?
                }
            }
            Core::Polygon(polygon) => {
                if r == 0 {
                    polyclip::union_all(polygon, FillRule::NonZero).map_err(geometry_error)?
                } else {
                    offset(polygon, r, Join::Round, tol).map_err(geometry_error)?
                }
            }
        };
        Ok(Area { polygons })
    }

    /// Whether there is nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }

    fn boolean(&self, op: Op, other: &Area) -> Result<Area, GeomError> {
        let polygons = Boolean::new()
            .subject(&self.polygons, FillRule::NonZero)
            .clip(&other.polygons, FillRule::NonZero)
            .op(op)
            .execute()
            .map_err(geometry_error)?;
        Ok(Area { polygons })
    }

    /// This with any of `others` taken away, in one operation.
    pub(crate) fn minus_all(&self, others: &[Area]) -> Result<Area, GeomError> {
        if others.iter().all(Area::is_empty) {
            return Ok(self.clone());
        }
        let clip: Vec<&Polygon> = others.iter().flat_map(|a| &a.polygons).collect();
        let clip: Vec<Polygon> = clip.into_iter().cloned().collect();
        let polygons = Boolean::new()
            .subject(&self.polygons, FillRule::NonZero)
            .clip(&clip, FillRule::NonZero)
            .op(Op::Difference)
            .execute()
            .map_err(geometry_error)?;
        Ok(Area { polygons })
    }

    /// Everything in any of `areas`.
    pub(crate) fn union_all(areas: &[Area]) -> Result<Area, GeomError> {
        let all: Vec<Polygon> = areas
            .iter()
            .flat_map(|a| a.polygons.iter().cloned())
            .collect();
        if all.is_empty() {
            return Ok(Area::empty());
        }
        let polygons = polyclip::union_all(&all, FillRule::NonZero).map_err(geometry_error)?;
        Ok(Area { polygons })
    }

    /// What this and `other` share.
    pub(crate) fn and(&self, other: &Area) -> Result<Area, GeomError> {
        self.boolean(Op::Intersection, other)
    }

    /// What one of the two has and the other doesn't.
    #[cfg(all(test, feature = "kicad"))]
    pub(crate) fn xor(&self, other: &Area) -> Result<Area, GeomError> {
        self.boolean(Op::Xor, other)
    }

    /// This without the parts narrower than `2 * d`: shrunk by `d` and grown again, with
    /// round corners, then kept within itself, so that it never reaches beyond it.
    pub(crate) fn opening(&self, d: i64, tolerance: i64) -> Result<Area, GeomError> {
        if d <= 0 || self.is_empty() {
            return Ok(self.clone());
        }
        let polygons = opening(&self.polygons, d, arc_tol(tolerance, ArcSide::Inside))
            .map_err(geometry_error)?;
        Area { polygons }.and(self)
    }

    /// The connected pieces, each a polygon with its holes, in a fixed order.
    pub(crate) fn pieces(&self) -> Vec<Area> {
        self.polygons
            .iter()
            .map(|p| Area {
                polygons: vec![p.clone()],
            })
            .collect()
    }

    /// Twice the area, exactly.
    pub(crate) fn area2(&self) -> i128 {
        self.polygons.iter().map(Polygon::signed_area2).sum()
    }

    /// The pieces as polygon shapes, with no radius.
    pub(crate) fn shapes(&self) -> Vec<super::shape::Shape> {
        self.polygons
            .iter()
            .map(|p| super::shape::Shape::new(Core::Polygon(p.clone()), 0, 0))
            .collect()
    }

    /// The pieces as rings, each piece's outline first and then its holes.
    pub(crate) fn rings(&self) -> Vec<Vec<Vec<Point>>> {
        self.polygons.iter().map(polygon_rings).collect()
    }

    /// Whether a point is inside, or on the boundary.
    pub(crate) fn contains_point(&self, p: Point) -> bool {
        let p = to_poly(p);
        self.polygons
            .iter()
            .any(|poly| polyclip::locate_in_polygon(poly, p) != polyclip::Location::Outside)
    }

    /// The box of every vertex, if there are any.
    pub(crate) fn bbox(&self) -> Option<super::types::Rect> {
        self.polygons
            .iter()
            .filter_map(Polygon::bbox)
            .map(|r| super::types::Rect::new(from_poly(r.min), from_poly(r.max)))
            .reduce(|a, b| a.union(&b))
    }
}

/// A polygon shape as one ring, with each hole joined to the outside by a cut of no width,
/// as KiCad saves zone fills. `None` for any other shape.
#[cfg(feature = "kicad")]
pub(crate) fn fractured(shape: &super::shape::Shape) -> Option<Vec<Point>> {
    match &shape.core {
        Core::Polygon(p) if shape.radius2 == 0 => polyclip::fracture(p)
            .ok()
            .map(|ring| ring.0.into_iter().map(from_poly).collect()),
        _ => None,
    }
}
