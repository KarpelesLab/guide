//! Shapes: a core, measured by polyclip, plus a radius.

use super::poly::Core;
use super::types::{GeomError, MAX_COORD, Point, Rect};

/// The default arc tolerance, in database units: 100 nm on a PCB, a fifth of the 0.5 µm by
/// which KiCad's DRC lets a clearance fall short.
pub const DEFAULT_ARC_TOLERANCE: i64 = 100;

/// A piece of geometry that clearances are measured to: a track, a pad, a via, a hole or a
/// keepout.
///
/// A shape is every point within a radius of its *core*, which is a point, a segment, a
/// polyline or a polygon. A track is a segment with half its width as the radius, and a
/// circle a point with a radius, so nothing round is ever turned into a polygon and these
/// shapes are exact. Only [arcs](Shape::arc_track) are approximated, which
/// [`is_exact`](Shape::is_exact) tells.
///
/// ```
/// use guide::geom::{Point, Shape};
///
/// let track = Shape::track(Point::new(0, 0), Point::new(1000, 0), 200)?;
/// let via = Shape::circle(Point::new(500, 600), 400)?;
/// // 600 between centerline and centre, less radii of 100 and 200, leaves exactly 300.
/// assert!(track.violates(&via, 301)?);
/// assert!(!track.violates(&via, 300)?);
/// # Ok::<(), guide::geom::GeomError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape {
    pub(super) core: Core,
    /// The radius, doubled, as a width may be odd.
    pub(super) radius2: i64,
    /// How far the true shape may reach beyond this one, doubled. Zero if exact.
    pub(super) error2: i64,
    /// The bounding box of the core.
    pub(super) core_bounds: Rect,
}

fn check_dimension(what: &'static str, value: i64) -> Result<i64, GeomError> {
    if value > 0 && value <= MAX_COORD {
        Ok(value)
    } else {
        Err(GeomError::InvalidDimension { what, value })
    }
}

impl Shape {
    fn new(core: Core, radius2: i64, error2: i64) -> Shape {
        let (min, max) = core.bounds();
        Shape {
            core,
            radius2,
            error2,
            core_bounds: Rect::new(min, max),
        }
    }

    /// A filled circle, such as a via or a round pad or hole. Exact.
    pub fn circle(center: Point, diameter: i64) -> Result<Shape, GeomError> {
        center.check()?;
        let diameter = check_dimension("circle diameter", diameter)?;
        Ok(Shape::new(Core::point(center), diameter, 0))
    }

    /// A track with round ends, or an oblong pad or slot: every point within `width / 2` of
    /// the segment from `a` to `b`, which may be one point. Exact.
    pub fn track(a: Point, b: Point, width: i64) -> Result<Shape, GeomError> {
        a.check()?;
        b.check()?;
        let width = check_dimension("track width", width)?;
        Ok(Shape::new(Core::segment(a, b), width, 0))
    }

    /// A track along the circular arc from `start` through `mid` to `end`, approximated.
    ///
    /// The centerline becomes a polyline within `tolerance` of the true arc, plus a unit
    /// for rounding its vertices to integers. Distances from it are reduced by that much,
    /// so that they are lower bounds: a [check](Shape::violates) can then report a
    /// violation that isn't one, but can't miss one that is. See the
    /// [module documentation](super#exactness).
    ///
    /// Collinear points make a straight track, still treated as approximate. If `start`
    /// equals `end`, this is a full circle with `mid` diametrically opposite.
    pub fn arc_track(
        start: Point,
        mid: Point,
        end: Point,
        width: i64,
        tolerance: i64,
    ) -> Result<Shape, GeomError> {
        start.check()?;
        mid.check()?;
        end.check()?;
        let width = check_dimension("track width", width)?;
        let tolerance = check_dimension("arc tolerance", tolerance)?;
        let core = Core::arc(start, mid, end, tolerance)?;
        // polyclip rounds each coordinate of a vertex to the nearest integer, which moves
        // it, and the polyline, by at most sqrt(2)/2. One unit covers that.
        Ok(Shape::new(core, width, 2 * (tolerance + 1)))
    }

    /// A filled polygon with holes, in any orientation. Exact.
    ///
    /// Rejected unless every ring is simple and the holes are properly nested.
    pub fn polygon(outer: &[Point], holes: &[&[Point]]) -> Result<Shape, GeomError> {
        Shape::rounded_polygon(outer, holes, 0)
    }

    /// A polygon grown by `radius`, which rounds its convex corners. Exact.
    pub fn rounded_polygon(
        outer: &[Point],
        holes: &[&[Point]],
        radius: i64,
    ) -> Result<Shape, GeomError> {
        for p in outer.iter().chain(holes.iter().copied().flatten()) {
            p.check()?;
        }
        if !(0..=MAX_COORD).contains(&radius) {
            return Err(GeomError::InvalidDimension {
                what: "polygon radius",
                value: radius,
            });
        }
        Ok(Shape::new(Core::polygon(outer, holes)?, 2 * radius, 0))
    }

    /// The area that `rings` enclose under the non-zero winding rule, as one polygon
    /// shape for each connected piece, with its holes, in a fixed order.
    ///
    /// Unlike [`polygon`](Shape::polygon), this takes rings in any orientation that
    /// touch, overlap and cross themselves and each other, such as the fractured outlines
    /// KiCad saves zone fills as: a hole joined to the outline by a cut of zero width. It
    /// is exact where rings only meet along shared edges and at their vertices, as those
    /// do. Where edges cross between vertices, the crossing is rounded to the nearest
    /// integer point, which moves the edges through it by up to `sqrt(2)/2`.
    ///
    /// ```
    /// use guide::geom::{Point, Shape};
    ///
    /// // A square with a square hole, joined along y = 4 from the hole to the edge. The
    /// // hole runs the other way round from the outline.
    /// let ring: Vec<Point> = [
    ///     (0, 0), (10, 0), (10, 4), (7, 4), (7, 3), (3, 3), (3, 7), (7, 7), (7, 4), (10, 4),
    ///     (10, 10), (0, 10),
    /// ]
    /// .iter()
    /// .map(|&(x, y)| Point::new(x, y))
    /// .collect();
    /// let pieces = Shape::region(&[&ring])?;
    /// assert_eq!(pieces.len(), 1);
    /// let inside_hole = Shape::circle(Point::new(5, 5), 2)?;
    /// assert!(!pieces[0].touches(&inside_hole));
    /// # Ok::<(), guide::geom::GeomError>(())
    /// ```
    pub fn region(rings: &[&[Point]]) -> Result<Vec<Shape>, GeomError> {
        for p in rings.iter().copied().flatten() {
            p.check()?;
        }
        Ok(Core::region(rings)?
            .into_iter()
            .map(|core| Shape::new(core, 0, 0))
            .collect())
    }

    /// A filled rectangle, whose sides must have positive length. Exact.
    pub fn rect(rect: Rect) -> Result<Shape, GeomError> {
        Shape::round_rect(rect, 0)
    }

    /// A rectangle with corners rounded to `radius`, at most half its shorter side. Exact,
    /// as it is a smaller rectangle grown by `radius`.
    pub fn round_rect(rect: Rect, radius: i64) -> Result<Shape, GeomError> {
        let (min, max) = (rect.min().check()?, rect.max().check()?);
        let (w, h) = (max.x - min.x, max.y - min.y);
        check_dimension("rectangle width", w)?;
        check_dimension("rectangle height", h)?;
        if radius < 0 || 2 * radius > w.min(h) {
            return Err(GeomError::InvalidDimension {
                what: "corner radius",
                value: radius,
            });
        }
        let (lo, hi) = (
            Point::new(min.x + radius, min.y + radius),
            Point::new(max.x - radius, max.y - radius),
        );
        let core = if lo.x == hi.x || lo.y == hi.y {
            // With a side used up by the corners, the rest is a segment or a point.
            Core::segment(lo, hi)
        } else {
            let corners = [lo, Point::new(hi.x, lo.y), hi, Point::new(lo.x, hi.y)];
            Core::polygon(&corners, &[])?
        };
        Ok(Shape::new(core, 2 * radius, 0))
    }

    /// Whether distances to this shape are exact. Only arcs' are not.
    #[inline]
    pub fn is_exact(&self) -> bool {
        self.error2 == 0
    }

    /// A rectangle holding the shape, with any arc error.
    pub fn bbox(&self) -> Rect {
        // Round a half up, so that the box holds it.
        let reach = (self.radius2 + self.error2 + 1) / 2;
        self.core_bounds.expand(reach)
    }
}
