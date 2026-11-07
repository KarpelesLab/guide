//! Exact conversions of decimal numbers in interchange files, and rotations that are the
//! same on every platform. Shared by the KiCad and Specctra readers.

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

/// `text`, a decimal number, times `factor * 10^scale`, rounded to the nearest integer with
/// halves away from zero. `None` if it isn't a number or is beyond `limit`.
///
/// `factor` is at most `10^8`, which leaves room in `u128` for the thirty digits kept.
pub(crate) fn scaled(text: &str, factor: u128, scale: i32, limit: i64) -> Option<i64> {
    debug_assert!(factor > 0 && factor <= 100_000_000);
    let d = decimal(text)?;
    let exp = d.exp.checked_add(scale)?;
    let digits = d.digits.checked_mul(factor)?;
    let magnitude: u128 = if digits == 0 {
        0
    } else if exp >= 0 {
        let power = 10u128.checked_pow(u32::try_from(exp).ok()?)?;
        digits.checked_mul(power)?
    } else {
        match 10u128.checked_pow(exp.unsigned_abs()) {
            Some(divisor) => {
                let (q, r) = (digits / divisor, digits % divisor);
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

/// An angle in millidegrees, normalised to `[0, 360000)`.
pub(crate) fn normalize(angle: i32) -> i32 {
    angle.rem_euclid(360_000)
}

/// Rotates a vector by `angle`, in millidegrees, counter-clockwise as seen on screen, in
/// coordinates whose Y runs down. This is KiCad's `RotatePoint`: exact for multiples of
/// 90°, and otherwise computed in `f64` and rounded half away from zero.
///
/// In coordinates whose Y runs up, it turns counter-clockwise too, as mirroring both the
/// vector and the result through the X axis shows.
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

/// Rotates a vector by `angle`, in millidegrees, counter-clockwise in coordinates whose Y
/// runs up, rounding as [`rotate`] does.
#[cfg_attr(not(feature = "dsn"), allow(dead_code))]
pub(crate) fn rotate_y_up(v: (i64, i64), angle: i32) -> (i64, i64) {
    let (x, y) = rotate((v.0, -v.1), angle);
    (x, -y)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_by_other_factors() {
        // Mils in nanometres: 25.4 µm each.
        assert_eq!(scaled("1", 254, 2, i64::MAX), Some(25_400));
        assert_eq!(scaled("-0.5", 254, 2, i64::MAX), Some(-12_700));
        assert_eq!(scaled("0.00001", 254, 2, i64::MAX), Some(0));
        assert_eq!(scaled("0.00002", 254, 2, i64::MAX), Some(1));
        // Inches.
        assert_eq!(scaled("0.1", 254, 5, i64::MAX), Some(2_540_000));
        assert_eq!(scaled("1", 1, 3, 999), None);
    }

    #[test]
    fn rotations_in_both_frames() {
        assert_eq!(rotate_y_up((1000, 0), 90_000), (0, 1000));
        assert_eq!(rotate_y_up((0, 1000), 90_000), (-1000, 0));
        assert_eq!(rotate_y_up((1_000_000, 0), 30_000), (866_025, 500_000));
    }
}
