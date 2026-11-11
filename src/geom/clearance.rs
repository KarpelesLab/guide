//! Distances between shapes, and whether they violate a clearance.
//!
//! Every question comes down to comparing the distance between two cores with `m / 2`, for
//! a whole number `m`: twice the clearance, plus the doubled radii and errors. The first of
//! these that can answer does:
//!
//! 1. the cores' bounding boxes, in integers;
//! 2. a filter in `f64`, which handles most pairs of tracks and vias without polyclip
//!    collecting segments into vectors;
//! 3. polyclip, which is exact, for even `m`;
//! 4. for odd `m`, polyclip once more, which leaves only a band of one unit undecided, as
//!    it compares with whole numbers. Then [`min_sq`] measures every pair of edges with
//!    exact rationals.

use core::cmp::Ordering;

use super::shape::Shape;
use super::types::{GeomError, MAX_COORD, Point};

impl Shape {
    /// Whether the gap between the two shapes is strictly less than `clearance`.
    ///
    /// A gap of exactly `clearance` is not a violation, and neither is anything with a
    /// `clearance` of zero: see [`touches`](Shape::touches) for that. With two exact shapes
    /// the answer is exact. With an [arc](Shape::arc_track), it is `true` whenever the true
    /// answer is, and may be `true` as well when the gap is above `clearance` by up to
    /// twice the arc's error bound.
    ///
    /// # Errors
    ///
    /// [`GeomError::InvalidDimension`] for a negative `clearance`, or one beyond
    /// [`MAX_COORD`](super::MAX_COORD).
    pub fn violates(&self, other: &Shape, clearance: i64) -> Result<bool, GeomError> {
        let m = threshold(self, other, clearance)?;
        if clearance == 0 || bounds_at_least(self, other, m) {
            return Ok(false);
        }
        Ok(match filter(self, other, m) {
            Filter::Below => true,
            Filter::NotBelow => false,
            Filter::Unknown => exact_below(self, other, m),
        })
    }

    /// Whether the two shapes share a point: they overlap, or touch at their boundaries.
    ///
    /// With an arc, this is `true` whenever the true answer is, and may be `true` as well
    /// when they are up to twice the arc's error bound apart.
    pub fn touches(&self, other: &Shape) -> bool {
        let m = reach2(self, other);
        if m == 0 {
            return self.core.intersects(&other.core);
        }
        if bounds_at_least(self, other, m + 1) {
            return false;
        }
        exact_cmp(self, other, m) != Ordering::Greater
    }

    /// The exact distance between the two shapes, zero if they touch or overlap.
    ///
    /// This costs much more than [`violates`](Shape::violates), which it agrees with.
    ///
    /// ```
    /// use core::cmp::Ordering;
    /// use guide::geom::{Point, Shape};
    ///
    /// // Centres 5 apart, by a 3-4-5 triangle, less radii of 1/2 and 1.
    /// let a = Shape::circle(Point::new(0, 0), 1)?;
    /// let b = Shape::circle(Point::new(3, 4), 2)?;
    /// let gap = a.distance(&b);
    /// assert_eq!((gap.floor(), gap.ceil()), (3, 4));
    /// assert_eq!(gap.cmp_to(3), Ordering::Greater);
    /// assert_eq!(gap.cmp_to(4), Ordering::Less);
    /// assert_eq!(gap.to_f64(), 3.5);
    /// # Ok::<(), guide::geom::GeomError>(())
    /// ```
    pub fn distance(&self, other: &Shape) -> Gap {
        Gap {
            core: min_sq(self, other),
            reach2: reach2(self, other),
            exact: self.is_exact() && other.is_exact(),
        }
    }
}

/// The doubled radii and errors of both shapes. Each is at most about 2^42, so neither this
/// nor the thresholds built on it can overflow.
fn reach2(a: &Shape, b: &Shape) -> i64 {
    a.radius2 + b.radius2 + a.error2 + b.error2
}

/// The doubled limit on the distance between cores for a clearance.
fn threshold(a: &Shape, b: &Shape, clearance: i64) -> Result<i64, GeomError> {
    if !(0..=MAX_COORD).contains(&clearance) {
        return Err(GeomError::InvalidDimension {
            what: "clearance",
            value: clearance,
        });
    }
    Ok(2 * clearance + reach2(a, b))
}

/// Whether the cores' bounding boxes prove their distance to be at least `m / 2`.
fn bounds_at_least(a: &Shape, b: &Shape, m: i64) -> bool {
    let m = m as u128;
    a.core_bounds.gap_sq(&b.core_bounds) * 4 >= m * m
}

/// The answer to "is the distance below `m / 2`?", as far as the filter can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
    Below,
    NotBelow,
    Unknown,
}

/// How far an `f64` distance is trusted, in units, which is generous.
///
/// With coordinates within 2^40, differences are within 2^41 and exact in `f64`. A cross
/// product of two such vectors `u` and `v` is off by about `2^-52 * |u| * |v|` at most, and
/// dividing by `|u|` for a distance leaves `2^-52 * |v|`, a thousandth of a unit. Squares,
/// square roots and the division add relative errors of the same size.
const FILTER_SLACK: f64 = 1.0;

/// Tries to decide whether the distance between the cores is below `m / 2`, in `f64`.
///
/// For a polygon, only "below" can be decided, by its edges: the other shape may lie inside
/// it, far from every edge.
pub(super) fn filter(a: &Shape, b: &Shape, m: i64) -> Filter {
    let limit = m as f64 / 2.0;
    let mut clear = !a.core.is_polygon() && !b.core.is_polygon();
    let below = a.core.any_edge(|p, q| {
        b.core.any_edge(|r, s| match edge_distance(p, q, r, s) {
            Some(d) if d + FILTER_SLACK <= limit => true,
            Some(d) => {
                if d - FILTER_SLACK < limit {
                    clear = false;
                }
                false
            }
            None => {
                clear = false;
                false
            }
        })
    });
    if below {
        Filter::Below
    } else if clear {
        Filter::NotBelow
    } else {
        Filter::Unknown
    }
}

/// The sign of `ux * vy - uy * vx`, or `None` if rounding could have changed it.
#[inline]
fn certain_sign(ux: f64, uy: f64, vx: f64, vy: f64) -> Option<bool> {
    let (l, r) = (ux * vy, uy * vx);
    let det = l - r;
    // The inputs are exact, so the error is at most an ulp or so of the larger product.
    let bound = (l.abs() + r.abs()) * (4.0 * f64::EPSILON);
    if det > bound {
        Some(true)
    } else if -det > bound {
        Some(false)
    } else {
        None
    }
}

/// The distance between two segments in `f64`, or `None` if rounding leaves it unclear
/// whether they cross.
///
/// The crossing test can't just trust the signs it computes: a crossing missed through a
/// wrong sign would give a large distance for segments that meet.
fn edge_distance(p: Point, q: Point, r: Point, s: Point) -> Option<f64> {
    let (px, py, qx, qy) = (p.x as f64, p.y as f64, q.x as f64, q.y as f64);
    let (rx, ry, sx, sy) = (r.x as f64, r.y as f64, s.x as f64, s.y as f64);
    if p != q && r != s {
        let (ux, uy) = (qx - px, qy - py);
        let (vx, vy) = (sx - rx, sy - ry);
        let side_r = certain_sign(ux, uy, rx - px, ry - py);
        let side_s = certain_sign(ux, uy, sx - px, sy - py);
        let side_p = certain_sign(vx, vy, px - rx, py - ry);
        let side_q = certain_sign(vx, vy, qx - rx, qy - ry);
        let apart =
            |a: Option<bool>, b: Option<bool>| matches!((a, b), (Some(a), Some(b)) if a == b);
        let across =
            |a: Option<bool>, b: Option<bool>| matches!((a, b), (Some(a), Some(b)) if a != b);
        if apart(side_r, side_s) || apart(side_p, side_q) {
            // One segment is strictly on one side of the other's line: no crossing.
        } else if across(side_r, side_s) && across(side_p, side_q) {
            return Some(0.0);
        } else {
            return None;
        }
    }
    let d = point_segment_f64(px, py, rx, ry, sx, sy)
        .min(point_segment_f64(qx, qy, rx, ry, sx, sy))
        .min(point_segment_f64(rx, ry, px, py, qx, qy))
        .min(point_segment_f64(sx, sy, px, py, qx, qy));
    Some(d)
}

#[inline]
fn point_segment_f64(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let (ex, ey) = (px - ax, py - ay);
    let t = dx * ex + dy * ey;
    let len2 = dx * dx + dy * dy;
    if t <= 0.0 {
        (ex * ex + ey * ey).sqrt()
    } else if t >= len2 {
        let (fx, fy) = (px - bx, py - by);
        (fx * fx + fy * fy).sqrt()
    } else {
        (dx * ey - dy * ex).abs() / len2.sqrt()
    }
}

/// Exactly whether the distance between the cores is below `m / 2`, without the filter.
pub(super) fn exact_below(a: &Shape, b: &Shape, m: i64) -> bool {
    if m % 2 == 0 {
        a.core.closer_than(&b.core, m / 2)
    } else {
        exact_cmp(a, b, m) == Ordering::Less
    }
}

/// Compares the distance between the cores with `m / 2`, exactly.
fn exact_cmp(a: &Shape, b: &Shape, m: i64) -> Ordering {
    // polyclip compares with whole numbers, and `n <= m / 2 < n + 1`. Equality, and
    // anything for odd `m`, takes more than that inside the band.
    let n = m / 2;
    if a.core.closer_than(&b.core, n) {
        return Ordering::Less;
    }
    if !a.core.closer_than(&b.core, n + 1) {
        return Ordering::Greater;
    }
    min_sq(a, b).cmp_half(m)
}

/// The exact squared distance between the cores.
///
/// polyclip computes this too, but only exposes it compared with whole numbers, which can't
/// tell whether a distance is below 2.5. So it is computed here in the open, by brute force.
fn min_sq(a: &Shape, b: &Shape) -> Ratio {
    if a.core.intersects(&b.core) {
        return Ratio::ZERO;
    }
    // The cores don't meet, so that neither lies inside the other, and their closest
    // points include an end of one of the edges.
    let mut best: Option<Ratio> = None;
    a.core.any_edge(|p, q| {
        b.core.any_edge(|r, s| {
            for d in [
                point_segment_sq(p, r, s),
                point_segment_sq(q, r, s),
                point_segment_sq(r, p, q),
                point_segment_sq(s, p, q),
            ] {
                if best.is_none_or(|x| d.cmp_ratio(&x) == Ordering::Less) {
                    best = Some(d);
                }
            }
            false
        })
    });
    best.unwrap_or(Ratio::ZERO)
}

/// The exact squared distance from a point to a segment. With coordinates within 2^40,
/// differences are within 2^41, and their products and sums within 2^83, as `i128` holds.
fn point_segment_sq(p: Point, a: Point, b: Point) -> Ratio {
    let (dx, dy) = (b.x as i128 - a.x as i128, b.y as i128 - a.y as i128);
    let (ex, ey) = (p.x as i128 - a.x as i128, p.y as i128 - a.y as i128);
    let t = dx * ex + dy * ey;
    let len2 = dx * dx + dy * dy;
    if t <= 0 {
        Ratio::whole((ex * ex + ey * ey) as u128)
    } else if t >= len2 {
        let (fx, fy) = (p.x as i128 - b.x as i128, p.y as i128 - b.y as i128);
        Ratio::whole((fx * fx + fy * fy) as u128)
    } else {
        // The distance to the line: |cross| / |d|, squared.
        let c = (dx * ey - dy * ex).unsigned_abs();
        Ratio {
            num: U256::mul(c, c),
            den: len2 as u128,
        }
    }
}

/// An unsigned 256-bit integer, just enough to compare the rationals here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct U256 {
    // In this order, so that the derived ordering is numeric.
    hi: u128,
    lo: u128,
}

impl U256 {
    /// The full product of two `u128`.
    fn mul(a: u128, b: u128) -> U256 {
        const LOW: u128 = u64::MAX as u128;
        let (a1, a0) = (a >> 64, a & LOW);
        let (b1, b0) = (b >> 64, b & LOW);
        let low = a0 * b0;
        let mid1 = a1 * b0 + (low >> 64);
        let mid2 = a0 * b1 + (mid1 & LOW);
        U256 {
            hi: a1 * b1 + (mid1 >> 64) + (mid2 >> 64),
            lo: (mid2 << 64) | (low & LOW),
        }
    }

    /// The product with a `u128`, which must fit: see [`Ratio`] for why it does.
    fn mul_u128(self, b: u128) -> U256 {
        let lo = U256::mul(self.lo, b);
        U256 {
            hi: self.hi.wrapping_mul(b).wrapping_add(lo.hi),
            lo: lo.lo,
        }
    }
}

/// A squared distance, `num / den`, exactly.
///
/// Numerators are below 2^166, as squares of cross products below 2^83, and denominators
/// below 2^83. Comparing two takes products below 2^249, and comparing one with `m^2 / 4`
/// for `m` below 2^50 takes products below 2^185.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ratio {
    num: U256,
    den: u128,
}

impl Ratio {
    const ZERO: Ratio = Ratio {
        num: U256 { hi: 0, lo: 0 },
        den: 1,
    };

    fn whole(value: u128) -> Ratio {
        Ratio {
            num: U256 { hi: 0, lo: value },
            den: 1,
        }
    }

    fn cmp_ratio(&self, other: &Ratio) -> Ordering {
        self.num
            .mul_u128(other.den)
            .cmp(&other.num.mul_u128(self.den))
    }

    /// Compares the square root of this with `m / 2`, for `0 <= m < 2^50`.
    fn cmp_half(&self, m: i64) -> Ordering {
        let m = m as u128;
        self.num.mul_u128(4).cmp(&U256::mul(m * m, self.den))
    }

    fn to_f64(self) -> f64 {
        let num = self.num.hi as f64 * 2f64.powi(128) + self.num.lo as f64;
        num / self.den as f64
    }
}

/// The minimum distance between two shapes, held exactly.
///
/// A distance is in general the square root of a fraction less half a whole number, which
/// no plain number type holds, so a `Gap` is compared with [`cmp_to`](Gap::cmp_to) and
/// rounded with [`floor`](Gap::floor) and [`ceil`](Gap::ceil). [`to_f64`](Gap::to_f64) is
/// for showing to people. If an arc was involved, [`is_exact`](Gap::is_exact) is `false`
/// and all of these describe a lower bound on the true gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    /// The squared distance between the cores.
    core: Ratio,
    /// The doubled radii and errors of both shapes.
    reach2: i64,
    exact: bool,
}

impl Gap {
    /// Whether this is the true distance, rather than a lower bound.
    #[inline]
    pub fn is_exact(&self) -> bool {
        self.exact
    }

    /// Compares the gap with a value, exactly.
    pub fn cmp_to(&self, value: i64) -> Ordering {
        if value <= 0 {
            return if value == 0 && self.is_zero() {
                Ordering::Equal
            } else {
                Ordering::Greater
            };
        }
        // Cores are less than 2^42 apart, so anything this large is beyond every gap.
        match value
            .checked_mul(2)
            .and_then(|v| v.checked_add(self.reach2))
        {
            Some(m) if m < (1 << 50) => self.core.cmp_half(m),
            _ => Ordering::Less,
        }
    }

    /// Whether the shapes touch or overlap.
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.core.cmp_half(self.reach2) != Ordering::Greater
    }

    /// Whether the shapes overlap, rather than at most touching.
    #[inline]
    pub fn is_overlap(&self) -> bool {
        self.core.cmp_half(self.reach2) == Ordering::Less
    }

    /// The largest whole number not above the gap.
    pub fn floor(&self) -> i64 {
        if self.is_zero() {
            return 0;
        }
        // Keeps `cmp_to(low) != Less` and `cmp_to(high) == Less`.
        let (mut low, mut high) = (0i64, 1i64 << 44);
        while high - low > 1 {
            let mid = low + (high - low) / 2;
            if self.cmp_to(mid) == Ordering::Less {
                high = mid;
            } else {
                low = mid;
            }
        }
        low
    }

    /// The smallest whole number not below the gap.
    pub fn ceil(&self) -> i64 {
        let floor = self.floor();
        if self.cmp_to(floor) == Ordering::Equal {
            floor
        } else {
            floor + 1
        }
    }

    /// The gap as an `f64`, for display. Don't decide anything with it.
    pub fn to_f64(&self) -> f64 {
        (self.core.to_f64().sqrt() - self.reach2 as f64 / 2.0).max(0.0)
    }
}
