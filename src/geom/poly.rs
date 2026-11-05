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

use polyclip::{ArcTol, Curve, FillRule, Geometry, Polygon, Ring, Side};

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
