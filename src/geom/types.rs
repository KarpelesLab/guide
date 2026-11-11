//! Coordinates, rectangles and the error type.

use core::fmt;

/// The largest coordinate magnitude, in database units: 2^40, polyclip's limit.
///
/// That is about 1.1 km in nanometres. Such values, and differences between them, convert
/// to `f64` exactly, which is what makes the filters in front of the exact checks sound.
pub const MAX_COORD: i64 = 1 << 40;

/// A point in database units (nanometres on a PCB), with Y pointing up.
///
/// The fields are public, so a `Point` can hold anything. Whatever takes points into
/// geometry checks them, and returns [`GeomError::CoordinateOutOfRange`] for those beyond
/// [`MAX_COORD`].
///
/// ```
/// use guide::geom::{GeomError, MAX_COORD, Point};
///
/// assert!(Point::checked(MAX_COORD, -MAX_COORD).is_ok());
/// assert!(matches!(
///     Point::checked(MAX_COORD + 1, 0),
///     Err(GeomError::CoordinateOutOfRange { .. })
/// ));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    /// Horizontal coordinate.
    pub x: i64,
    /// Vertical coordinate, increasing upward.
    pub y: i64,
}

impl Point {
    /// Creates a point without checking its range.
    #[inline]
    pub const fn new(x: i64, y: i64) -> Point {
        Point { x, y }
    }

    /// Creates a point, checking its range.
    ///
    /// # Errors
    ///
    /// [`GeomError::CoordinateOutOfRange`] if it is beyond [`MAX_COORD`].
    pub const fn checked(x: i64, y: i64) -> Result<Point, GeomError> {
        let p = Point { x, y };
        if p.in_range() {
            Ok(p)
        } else {
            Err(GeomError::CoordinateOutOfRange { x, y })
        }
    }

    /// Whether both coordinates are within [`MAX_COORD`].
    #[inline]
    pub const fn in_range(self) -> bool {
        self.x >= -MAX_COORD && self.x <= MAX_COORD && self.y >= -MAX_COORD && self.y <= MAX_COORD
    }

    /// Returns the point if it is in range.
    pub(crate) const fn check(self) -> Result<Point, GeomError> {
        Point::checked(self.x, self.y)
    }
}

/// An axis-aligned rectangle, closed: its boundary belongs to it.
///
/// ```
/// use guide::geom::{Point, Rect};
///
/// let r = Rect::new(Point::new(10, 0), Point::new(0, 5));
/// assert_eq!(r.min(), Point::new(0, 0));
/// assert_eq!(r.max(), Point::new(10, 5));
/// // Touching counts.
/// assert!(r.intersects(&Rect::new(Point::new(10, 5), Point::new(20, 20))));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    min: Point,
    max: Point,
}

impl Rect {
    /// The smallest rectangle holding two points, given in any order.
    #[inline]
    pub const fn new(a: Point, b: Point) -> Rect {
        let (x0, x1) = if a.x < b.x { (a.x, b.x) } else { (b.x, a.x) };
        let (y0, y1) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
        Rect {
            min: Point::new(x0, y0),
            max: Point::new(x1, y1),
        }
    }

    /// The lower-left corner.
    #[inline]
    pub const fn min(&self) -> Point {
        self.min
    }

    /// The upper-right corner.
    #[inline]
    pub const fn max(&self) -> Point {
        self.max
    }

    /// Whether the two rectangles share a point. Touching counts.
    #[inline]
    pub const fn intersects(&self, other: &Rect) -> bool {
        self.min.x <= other.max.x
            && other.min.x <= self.max.x
            && self.min.y <= other.max.y
            && other.min.y <= self.max.y
    }

    /// The smallest rectangle holding both.
    #[inline]
    #[must_use]
    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            min: Point::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    /// The rectangle grown by `d` on every side. Saturates rather than overflowing.
    #[inline]
    #[must_use]
    pub fn expand(&self, d: i64) -> Rect {
        Rect {
            min: Point::new(self.min.x.saturating_sub(d), self.min.y.saturating_sub(d)),
            max: Point::new(self.max.x.saturating_add(d), self.max.y.saturating_add(d)),
        }
    }

    /// The area, in `i128` as a board-sized product overflows `i64`. It saturates, as an
    /// index takes any rectangle, and one as wide as `i64` overflows even `i128`.
    #[inline]
    pub(crate) fn area(&self) -> i128 {
        let w = self.max.x as i128 - self.min.x as i128;
        let h = self.max.y as i128 - self.min.y as i128;
        w.saturating_mul(h)
    }

    /// The squared gap between two rectangles, zero if they touch or overlap. Exact for
    /// rectangles within [`MAX_COORD`].
    pub(crate) fn gap_sq(&self, other: &Rect) -> u128 {
        let dx = (other.min.x as i128 - self.max.x as i128)
            .max(self.min.x as i128 - other.max.x as i128)
            .max(0) as u128;
        let dy = (other.min.y as i128 - self.max.y as i128)
            .max(self.min.y as i128 - other.max.y as i128)
            .max(0) as u128;
        dx * dx + dy * dy
    }
}

/// Why geometry was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GeomError {
    /// A coordinate is beyond [`MAX_COORD`].
    CoordinateOutOfRange {
        /// The horizontal coordinate.
        x: i64,
        /// The vertical coordinate.
        y: i64,
    },
    /// A width, diameter, radius, clearance or tolerance is outside what it may be.
    InvalidDimension {
        /// What the value is.
        what: &'static str,
        /// The value.
        value: i64,
    },
    /// A polygon is not simple, or its holes are not properly nested.
    InvalidPolygon {
        /// What is wrong with it.
        reason: String,
    },
    /// An arc could not be approximated, as it would bulge beyond [`MAX_COORD`] or take too
    /// many vertices at the tolerance asked for.
    InvalidArc {
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for GeomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeomError::CoordinateOutOfRange { x, y } => {
                write!(f, "coordinate ({x}, {y}) is beyond the limit of ±2^40")
            }
            GeomError::InvalidDimension { what, value } => write!(f, "invalid {what}: {value}"),
            GeomError::InvalidPolygon { reason } => write!(f, "invalid polygon: {reason}"),
            GeomError::InvalidArc { reason } => write!(f, "invalid arc: {reason}"),
        }
    }
}

impl std::error::Error for GeomError {}
