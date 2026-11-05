//! Exact conversions of KiCad's decimal numbers, and rotations as KiCad does them.

use crate::geom::{MAX_COORD, Point};

/// A decimal number, as `digits * 10^exp`, negative if `negative`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decimal {
    negative: bool,
    digits: u128,
    exp: i32,
}

/// The most significant digits kept. More can only matter far below a nanometre.
const MAX_DIGITS: usize = 30;

fn decimal(text: &str) -> Option<Decimal> {
    let (negative, rest) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let (mantissa, exp) = match rest.find(['e', 'E']) {
        Some(at) => (&rest[..at], rest[at + 1..].parse::<i32>().ok()?),
        None => (rest, 0),
    };
    let (whole, fraction) = match mantissa.find('.') {
        Some(at) => (&mantissa[..at], &mantissa[at + 1..]),
        None => (mantissa, ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let mut digits: u128 = 0;
    let mut kept = 0;
    let mut exp = exp.checked_sub(i32::try_from(fraction.len()).ok()?)?;
    for c in whole.bytes().chain(fraction.bytes()) {
        if !c.is_ascii_digit() {
            return None;
        }
        if digits == 0 && c == b'0' {
            continue;
        }
        if kept < MAX_DIGITS {
            digits = digits * 10 + u128::from(c - b'0');
            kept += 1;
        } else {
            // A dropped digit still scales the number. Dropping, rather than rounding,
            // can't change how the result rounds: what is dropped is less than a unit of
            // the last digit kept, far below the unit rounded to.
            exp = exp.checked_add(1)?;
        }
    }
    Some(Decimal {
        negative,
        digits,
        exp,
    })
}

/// `text`, a decimal number, times `10^scale`, rounded to the nearest integer with halves
/// away from zero, which is how KiCad rounds. `None` if it isn't a number or is beyond
/// `limit`.
fn scaled(text: &str, scale: i32, limit: i64) -> Option<i64> {
    let d = decimal(text)?;
    let exp = d.exp.checked_add(scale)?;
    let magnitude: u128 = if d.digits == 0 {
        0
    } else if exp >= 0 {
        let factor = 10u128.checked_pow(u32::try_from(exp).ok()?)?;
        d.digits.checked_mul(factor)?
    } else {
        match 10u128.checked_pow(exp.unsigned_abs()) {
            Some(divisor) => {
                let (q, r) = (d.digits / divisor, d.digits % divisor);
                // A half or more rounds away from zero.
                q + u128::from(r >= divisor - r)
            }
            // A divisor beyond u128 is beyond any digits: the number rounds to zero.
            None => 0,
        }
    };
    let magnitude = i64::try_from(magnitude).ok().filter(|&m| m <= limit)?;
    Some(if d.negative { -magnitude } else { magnitude })
}

/// Millimetres, as KiCad writes them, in nanometres. Exact up to six decimals, which is all
/// KiCad writes, and rounded half away from zero beyond that. `None` beyond
/// [`MAX_COORD`].
pub(crate) fn mm_to_nm(text: &str) -> Option<i64> {
    scaled(text, 6, MAX_COORD)
}

/// Degrees in millidegrees, rounded half away from zero. `None` beyond ±360°, which KiCad
/// never writes.
pub(crate) fn degrees_to_millidegrees(text: &str) -> Option<i32> {
    scaled(text, 3, 360_000).map(|v| v as i32)
}

/// A ratio, such as a rounded rectangle's, in millionths, rounded half away from zero.
pub(crate) fn ratio_to_millionths(text: &str) -> Option<i64> {
    scaled(text, 6, 1_000_000_000)
}

/// `v` times `millionths / 10^6`, rounded half away from zero, as KiCad's `KiROUND` would
/// round the product of a length and a ratio. Exact, where KiCad goes through `f64`.
pub(crate) fn times_ratio(v: i64, millionths: i64) -> i64 {
    let p = i128::from(v) * i128::from(millionths);
    let (q, r) = (p / 1_000_000, p % 1_000_000);
    let round = i128::from(2 * r.abs() >= 1_000_000) * p.signum();
    (q + round) as i64
}

/// An angle in millidegrees, normalised to `[0, 360000)`.
pub(crate) fn normalize(angle: i32) -> i32 {
    angle.rem_euclid(360_000)
}

/// Rotates a vector by `angle`, in millidegrees, counter-clockwise as seen on screen, in
/// KiCad's coordinates, whose Y runs down. This is KiCad's `RotatePoint`: exact for
/// multiples of 90°, and otherwise computed in `f64` and rounded half away from zero.
///
/// The sine and cosine come from [`sin_cos`], here, which uses only basic arithmetic and is
/// the same on every platform. KiCad uses the C library's, which may differ in the last bit,
/// and so may round a coordinate the other way in rare cases: a nanometre at most.
pub(crate) fn rotate(v: (i64, i64), angle: i32) -> (i64, i64) {
    let (x, y) = v;
    match normalize(angle) {
        0 => (x, y),
        90_000 => (y, -x),
        180_000 => (-x, -y),
        270_000 => (-y, x),
        a => {
            let (sin, cos) = sin_cos(a);
            let (fx, fy) = (x as f64, y as f64);
            (round(fy * sin + fx * cos), round(fy * cos - fx * sin))
        }
    }
}

/// Rounds half away from zero, as KiCad's `KiROUND`.
fn round(v: f64) -> i64 {
    v.round() as i64
}

/// The sine and cosine of an angle in millidegrees, from Taylor series on at most 45°,
/// in plain `f64` arithmetic so as to be the same on every platform.
pub(crate) fn sin_cos(angle: i32) -> (f64, f64) {
    let a = normalize(angle);
    // Reduce to [0, 90°) exactly, in integers.
    let (quadrant, a) = (a / 90_000, a % 90_000);
    // Then to [0, 45°], swapping sine and cosine above 45°.
    let (s, c) = if a <= 45_000 {
        taylor(a)
    } else {
        let (s, c) = taylor(90_000 - a);
        (c, s)
    };
    match quadrant {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// The sine and cosine of an angle in millidegrees, at most 45°.
fn taylor(angle: i32) -> (f64, f64) {
    let x = f64::from(angle) * (core::f64::consts::PI / 180_000.0);
    let x2 = x * x;
    // Terms to x^19 and x^18. The next ones are below 2^-70 at 45°.
    let (mut sin, mut cos) = (0.0, 0.0);
    let (mut s_term, mut c_term) = (x, 1.0);
    for n in 0..10 {
        sin += s_term;
        cos += c_term;
        let k = f64::from(2 * n + 2);
        s_term *= -x2 / (k * (k + 1.0));
        c_term *= -x2 / ((k - 1.0) * k);
    }
    (sin, cos)
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
