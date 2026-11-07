//! Exact conversions of KiCad's decimal numbers, and rotations as KiCad does them.

use crate::geom::{MAX_COORD, Point};
use crate::units::scaled;

pub(crate) use crate::units::rotate;
#[cfg(test)]
use crate::units::sin_cos;

/// Millimetres, as KiCad writes them, in nanometres. Exact up to six decimals, which is all
/// KiCad writes, and rounded half away from zero beyond that, which is how KiCad rounds.
/// `None` beyond [`MAX_COORD`].
pub(crate) fn mm_to_nm(text: &str) -> Option<i64> {
    scaled(text, 1, 6, MAX_COORD)
}

/// Degrees in millidegrees, rounded half away from zero. `None` beyond ±360°, which KiCad
/// never writes.
pub(crate) fn degrees_to_millidegrees(text: &str) -> Option<i32> {
    scaled(text, 1, 3, 360_000).map(|v| v as i32)
}

/// A ratio, such as a rounded rectangle's, in millionths, rounded half away from zero.
pub(crate) fn ratio_to_millionths(text: &str) -> Option<i64> {
    scaled(text, 1, 6, 1_000_000_000)
}

/// `v` times `millionths / 10^6`, rounded half away from zero, as KiCad's `KiROUND` would
/// round the product of a length and a ratio. Exact, where KiCad goes through `f64`.
pub(crate) fn times_ratio(v: i64, millionths: i64) -> i64 {
    let p = i128::from(v) * i128::from(millionths);
    let (q, r) = (p / 1_000_000, p % 1_000_000);
    let round = i128::from(2 * r.abs() >= 1_000_000) * p.signum();
    (q + round) as i64
}

/// A point in KiCad's coordinates, whose Y runs down, as a point of `guide`'s, whose Y runs
/// up: the same picture, mirrored through the X axis.
pub(crate) fn to_point(v: (i64, i64)) -> Point {
    Point::new(v.0, -v.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millimetres_are_exact() {
        for (text, nm) in [
            ("0", 0),
            ("1", 1_000_000),
            ("-1.5", -1_500_000),
            ("0.000001", 1),
            ("0.0000005", 1),
            ("-0.0000005", -1),
            ("0.00000049", 0),
            ("123.456789", 123_456_789),
            ("+2.", 2_000_000),
            (".25", 250_000),
            ("1e-3", 1_000),
            ("1.5E2", 150_000_000),
            ("0.15000000000000002", 150_000),
            ("1099511.627776", MAX_COORD),
            ("00012.5000", 12_500_000),
        ] {
            assert_eq!(mm_to_nm(text), Some(nm), "{text}");
        }
        for text in [
            "",
            "-",
            ".",
            "1.2.3",
            "abc",
            "1e",
            "1099511.627777",
            "nan",
            "1e999",
        ] {
            assert_eq!(mm_to_nm(text), None, "{text}");
        }
        // Digits beyond the 30 kept round correctly, in both parts.
        assert_eq!(mm_to_nm("0.0000014999999999999999999999999999999"), Some(1));
        assert_eq!(
            mm_to_nm("1000.0000000000000000000000000000000001"),
            Some(1_000_000_000)
        );
        assert_eq!(mm_to_nm("1000000000000000000000000000000000"), None);
    }

    #[test]
    fn angles_and_ratios() {
        assert_eq!(degrees_to_millidegrees("90"), Some(90_000));
        assert_eq!(degrees_to_millidegrees("-12.3456"), Some(-12_346));
        assert_eq!(degrees_to_millidegrees("360.1"), None);
        assert_eq!(ratio_to_millionths("0.25"), Some(250_000));
        assert_eq!(times_ratio(1_500_000, 250_000), 375_000);
        assert_eq!(times_ratio(3, 500_000), 2);
        assert_eq!(times_ratio(-3, 500_000), -2);
        assert_eq!(times_ratio(5, 100_000), 1);
    }

    #[test]
    fn rotations_match_kicad() {
        // KiCad's RotatePoint: 90° takes +X (right) to -Y, which is up on screen.
        assert_eq!(rotate((1000, 0), 90_000), (0, -1000));
        assert_eq!(rotate((1000, 0), -90_000), (0, 1000));
        assert_eq!(rotate((1000, 2000), 180_000), (-1000, -2000));
        assert_eq!(rotate((1000, 2000), 450_000 - 360_000), (2000, -1000));
        assert_eq!(rotate((1_000_000, 0), 45_000), (707_107, -707_107));
        assert_eq!(rotate((1_000_000, 0), 30_000), (866_025, -500_000));
        assert_eq!(rotate((0, 1_000_000), 30_000), (500_000, 866_025));
        for angle in (0..360_000).step_by(997) {
            let (s, c) = sin_cos(angle);
            let r = (angle as f64 / 1000.0).to_radians();
            assert!(
                (s - r.sin()).abs() < 1e-15 && (c - r.cos()).abs() < 1e-15,
                "{angle}"
            );
        }
    }
}
