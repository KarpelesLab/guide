//! Platform-independent math for code that affects results.
//!
//! Rust documents that [`f64::exp`] and the other transcendental functions can give
//! different answers on different platforms, because they call the platform's math
//! library. A single differing bit in an annealer's acceptance test sends the whole run
//! down another path, so anything that affects results uses the functions here instead.
//!
//! They are built only from `+ - * /` in a fixed order and from bit manipulation, which
//! IEEE 754 specifies exactly and Rust does not contract into fused operations. The
//! exception is 32-bit x86 without SSE2 (`i586` targets), where the x87 unit rounds
//! twice, which Rust documents as a known deviation. Same-seed reproducibility cannot be
//! promised on such targets by this or any other float code.

/// 1 / ln 2, used only to choose `k`, so its rounding does not reach the result.
const INV_LN2: f64 = f64::from_bits(0x3FF7_1547_652B_82FE);

/// The upper bits of ln 2, with 21 trailing zero bits so that `k * LN2_HI` is exact for
/// every `k` that can occur (at most 11 bits).
const LN2_HI: f64 = f64::from_bits(0x3FE6_2E42_FEE0_0000);

/// ln 2 - `LN2_HI`, the part of ln 2 that does not fit in `LN2_HI`.
const LN2_LO: f64 = f64::from_bits(0x3DEA_39EF_3579_3C76);

/// The largest `x` for which e^x rounds to a finite value.
const OVERFLOW_ABOVE: f64 = f64::from_bits(0x4086_2E42_FEFA_39EF);

/// The smallest `x` for which e^x does not round to zero. The next float down is below
/// ln(2^-1075), half of the smallest subnormal.
const UNDERFLOW_BELOW: f64 = f64::from_bits(0xC087_4910_D52D_3051);

/// Taylor coefficients 1/13! down to 1/3! and then 1/2!, highest order first.
///
/// Degree 13 is the lowest that is accurate enough. On the reduced range, the first
/// omitted term `r^14 / 14!` is at most 4e-18, which is 0.04 ULP of a result near one,
/// while stopping at degree 12 would leave about 1.7e-16, more than a whole ULP. A
/// minimax polynomial could save a term, but these can be checked by eye.
const COEFFICIENTS: [f64; 12] = [
    1.0 / 6_227_020_800.0,
    1.0 / 479_001_600.0,
    1.0 / 39_916_800.0,
    1.0 / 3_628_800.0,
    1.0 / 362_880.0,
    1.0 / 40_320.0,
    1.0 / 5_040.0,
    1.0 / 720.0,
    1.0 / 120.0,
    1.0 / 24.0,
    1.0 / 6.0,
    1.0 / 2.0,
];

/// Computes e^`x`, giving the same bits on every platform.
///
/// Use this rather than [`f64::exp`] for anything that affects results, such as the
/// annealer's acceptance test `exp(-delta / temperature)`.
///
/// Results are not always correctly rounded, so they can differ from [`f64::exp`] even
/// where that is accurate. The largest error measured over a million inputs across the
/// whole range was 0.89 ULP, against a value computed to 80 digits, and about 92% of
/// results were correctly rounded. That is a measurement and not a proven bound.
///
/// `exp` has also never been seen to decrease, in walks over hundreds of millions of
/// adjacent inputs, including around every point where the range reduction changes. That
/// is likewise tested rather than proven.
///
/// # Special values
///
/// | `x` | Result |
/// | --- | --- |
/// | NaN | NaN |
/// | positive infinity, or above about 709.78 | positive infinity |
/// | negative infinity, or below about -745.13 | `0.0` |
/// | between about -745.13 and -708.40 | a subnormal, correctly scaled |
/// | `0.0` or `-0.0` | `1.0` |
///
/// An acceptance test with a temperature of zero gets `-inf` for a positive `delta`, so a
/// result of `0.0`, but gets NaN for `delta == 0.0`. NaN compares false with everything,
/// so the test `rng.unit() < exp(...)` rejects the move rather than panicking.
///
/// # Examples
///
/// ```
/// use guide::math::exp;
///
/// assert_eq!(exp(0.0), 1.0);
/// assert_eq!(exp(f64::NEG_INFINITY), 0.0);
/// assert_eq!(exp(1000.0), f64::INFINITY);
/// assert!(exp(f64::NAN).is_nan());
///
/// // Within one ULP of e.
/// assert!((exp(1.0) - core::f64::consts::E).abs() <= 4.5e-16);
/// ```
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > OVERFLOW_ABOVE {
        return f64::INFINITY;
    }
    if x < UNDERFLOW_BELOW {
        return 0.0;
    }

    // Write x = k * ln2 + r with |r| <= about ln2 / 2, so that e^x = 2^k * e^r.
    let k = nearest_multiple_of_ln2(x);

    // ln 2 in two pieces keeps the digits that a single constant would lose. `hi` is exact
    // because k * LN2_HI is exact and its difference from x needs no extra bits.
    let hi = x - f64::from(k) * LN2_HI;
    let lo = f64::from(k) * LN2_LO;
    let r = hi - lo;

    // e^r = 1 + r + r^2 * q(r). The correction `lo` joins at the last step, added to the
    // small term rather than to r, so that its low bits still count when everything is
    // summed. This is what holds the result within one ULP.
    let mut q = 0.0;
    for coefficient in COEFFICIENTS {
        q = q * r + coefficient;
    }
    let y = 1.0 + (hi - (lo - r * r * q));

    scale_by_power_of_two(y, k)
}

/// Returns the integer `k` nearest to `x / ln 2`, for `x` within the bounds that [`exp`]
/// has already checked, which keep the value well inside `i32`.
fn nearest_multiple_of_ln2(x: f64) -> i32 {
    // The cast truncates, so adding a signed one half rounds to the nearest.
    let half = if x < 0.0 { -0.5 } else { 0.5 };
    (x * INV_LN2 + half) as i32
}

/// Returns `y * 2^k` with a single rounding, for `y` in about `[0.7, 1.42]` and `k` in the
/// range that [`exp`] can produce, `[-1075, 1024]`.
fn scale_by_power_of_two(y: f64, k: i32) -> f64 {
    // Far enough below the smallest normal that the first step of a two-step scaling,
    // with `k` at its lowest, stays normal.
    const STEP: i32 = 64;

    if k > 1023 {
        // 2^1024 is not representable, though y * 2^1024 can be when y < 1.
        y * 2.0 * power_of_two(k - 1)
    } else if k >= -1022 {
        y * power_of_two(k)
    } else {
        // 2^k is not a normal number here. The first multiplication is exact, so only the
        // second rounds, and a subnormal result does not suffer double rounding.
        y * power_of_two(k + STEP) * power_of_two(-STEP)
    }
}

/// Returns `2^k` for `k` in `[-1022, 1023]`, the exponents of normal numbers.
fn power_of_two(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The inputs where results run out of range: e^LOWEST_NONZERO rounds to the smallest
    /// subnormal, and e^HIGHEST_FINITE is finite.
    const LOWEST_NONZERO: f64 = UNDERFLOW_BELOW;
    const HIGHEST_FINITE: f64 = OVERFLOW_ABOVE;

    /// Well inside the inputs with subnormal results, which start at ln(2^-1022), about
    /// -708.3964. The margin keeps the input's rounding from mattering.
    const SURELY_SUBNORMAL_BELOW: f64 = -708.3965;

    /// Allowed distance from the platform's `f64::exp`. Measured against glibc this is at
    /// most 1. The platform's function has its own error, and platforms differ in it, so
    /// this leaves room. `GOLDEN` is the tighter check, and does not rely on the platform.
    const STD_TOLERANCE_ULPS: u64 = 3;

    /// The distance between two non-negative values, in representable steps. Consecutive
    /// floats have consecutive bits, across subnormals and up to infinity.
    fn ulps(a: f64, b: f64) -> u64 {
        a.to_bits().abs_diff(b.to_bits())
    }

    /// Input, then our result's bits, then the bits of the correctly rounded result.
    ///
    /// The second column holds regression values: they came from this implementation, and
    /// they only say that it has not changed. The third came from Python's `decimal` module
    /// computing e^x to 80 digits, then rounding that to `f64`.
    #[rustfmt::skip]
    const GOLDEN: [(f64, u64, u64); 35] = [
        (1.0, 0x4005_BF0A_8B14_576A, 0x4005_BF0A_8B14_5769),
        (-1.0, 0x3FD7_8B56_362C_EF38, 0x3FD7_8B56_362C_EF38),
        (0.5, 0x3FFA_6129_8E1E_069C, 0x3FFA_6129_8E1E_069C),
        (-0.5, 0x3FE3_68B2_FC6F_960A, 0x3FE3_68B2_FC6F_960A),
        (0.1, 0x3FF1_AEC7_B35A_00D4, 0x3FF1_AEC7_B35A_00D4),
        (-0.1, 0x3FEC_F46D_99D5_2B3A, 0x3FEC_F46D_99D5_2B3A),
        (2.5, 0x4028_5D6F_D931_E0BB, 0x4028_5D6F_D931_E0BB),
        (-2.5, 0x3FB5_0385_C094_F425, 0x3FB5_0385_C094_F425),
        (0.001, 0x3FF0_0419_19B7_EE34, 0x3FF0_0419_19B7_EE34),
        (-0.001, 0x3FEF_F7CF_E56F_1A9E, 0x3FEF_F7CF_E56F_1A9E),
        (1e-10, 0x3FF0_0000_0006_DF38, 0x3FF0_0000_0006_DF38),
        (-1e-10, 0x3FEF_FFFF_FFF2_4190, 0x3FEF_FFFF_FFF2_4190),
        (1e-300, 0x3FF0_0000_0000_0000, 0x3FF0_0000_0000_0000),
        // Either side of ln2 / 2, then ln 2 and 3/2 ln 2.
        (0.34657359027997264, 0x3FF6_A09E_667F_3BCC, 0x3FF6_A09E_667F_3BCC),
        (0.3465735902799727, 0x3FF6_A09E_667F_3BCD, 0x3FF6_A09E_667F_3BCD),
        (core::f64::consts::LN_2, 0x4000_0000_0000_0000, 0x4000_0000_0000_0000),
        (1.0397207708399179, 0x4006_A09E_667F_3BCC, 0x4006_A09E_667F_3BCC),
        (10.0, 0x40D5_829D_CF95_0560, 0x40D5_829D_CF95_0560),
        (-10.0, 0x3F07_CD79_B564_7C9A, 0x3F07_CD79_B564_7C9B),
        (100.0, 0x48F3_494A_9B17_1BF5, 0x48F3_494A_9B17_1BF5),
        (-100.0, 0x36EA_8C1F_14E2_AF5D, 0x36EA_8C1F_14E2_AF5D),
        (700.0, 0x7F0D_945D_F4F8_EC8E, 0x7F0D_945D_F4F8_EC8E),
        (-700.0, 0x00D1_4F2B_0FB9_307F, 0x00D1_4F2B_0FB9_307F),
        (709.0, 0x7FDD_422D_2BE5_DC9B, 0x7FDD_422D_2BE5_DC9B),
        (709.7, 0x7FED_75AE_7A50_EE14, 0x7FED_75AE_7A50_EE14),
        (HIGHEST_FINITE, 0x7FEF_FFFF_FFFF_FF2A, 0x7FEF_FFFF_FFFF_FF2A),
        // The last normal result, then subnormal ones, down to the smallest.
        (-708.3, 0x0011_9E98_B83D_E7A3, 0x0011_9E98_B83D_E7A3),
        (-708.4, 0x000F_F15B_469E_DF89, 0x000F_F15B_469E_DF89),
        (-708.5, 0x000E_6CF6_D088_97AC, 0x000E_6CF6_D088_97AC),
        (-720.0, 0x0000_0009_93B4_DC95, 0x0000_0009_93B4_DC95),
        (-740.0, 0x0000_0000_0000_0055, 0x0000_0000_0000_0055),
        (-744.0, 0x0000_0000_0000_0002, 0x0000_0000_0000_0002),
        (-745.0, 0x0000_0000_0000_0001, 0x0000_0000_0000_0001),
        (-745.13, 0x0000_0000_0000_0001, 0x0000_0000_0000_0001),
        (LOWEST_NONZERO, 0x0000_0000_0000_0001, 0x0000_0000_0000_0001),
    ];

    #[test]
    fn results_match_regression_values() {
        for (x, expected, _) in GOLDEN {
            assert_eq!(exp(x).to_bits(), expected, "exp({x:e})");
        }
    }

    /// Independent of the platform's math library, unlike the comparisons below.
    #[test]
    fn results_are_within_one_ulp_of_correctly_rounded() {
        for (x, _, correct) in GOLDEN {
            let distance = exp(x).to_bits().abs_diff(correct);
            assert!(distance <= 1, "exp({x:e}) is off by {distance} ULP");
        }
    }

    #[test]
    fn special_values() {
        assert_eq!(exp(0.0).to_bits(), 1.0f64.to_bits());
        assert_eq!(exp(-0.0).to_bits(), 1.0f64.to_bits());
        assert!(exp(f64::NAN).is_nan());
        assert!(exp(-f64::NAN).is_nan());
        assert_eq!(exp(f64::INFINITY), f64::INFINITY);
        assert_eq!(exp(f64::NEG_INFINITY).to_bits(), 0.0f64.to_bits());
        assert_eq!(exp(f64::MAX), f64::INFINITY);
        assert_eq!(exp(f64::MIN).to_bits(), 0.0f64.to_bits());

        // Inputs too small to move the result.
        for tiny in [f64::MIN_POSITIVE, 5e-324, 1e-20, -5e-324, -1e-20] {
            assert_eq!(exp(tiny), 1.0, "exp({tiny:e})");
        }
    }

    #[test]
    fn overflow_starts_exactly_where_the_true_result_rounds_to_infinity() {
        assert!(exp(HIGHEST_FINITE).is_finite());
        assert_eq!(exp(HIGHEST_FINITE.next_up()), f64::INFINITY);
    }

    #[test]
    fn underflow_starts_exactly_where_the_true_result_rounds_to_zero() {
        assert_eq!(exp(LOWEST_NONZERO).to_bits(), 1);
        assert_eq!(exp(LOWEST_NONZERO.next_down()).to_bits(), 0);
    }

    /// `LN2_HI` must have spare bits for `k * LN2_HI` to be exact, and the two pieces must
    /// add up to ln 2.
    #[test]
    fn constants_are_consistent() {
        assert!(LN2_HI.to_bits().trailing_zeros() >= 21);
        assert_eq!(LN2_HI + LN2_LO, core::f64::consts::LN_2);
        assert_eq!(INV_LN2, core::f64::consts::LOG2_E);
    }

    /// A deterministic sweep, to back up the random inputs of the property tests.
    #[test]
    fn sweep_agrees_with_std() {
        let steps = 400_000;
        let step = (HIGHEST_FINITE - LOWEST_NONZERO) / f64::from(steps);
        for i in 0..=steps {
            let x = (LOWEST_NONZERO + f64::from(i) * step).min(HIGHEST_FINITE);
            let distance = ulps(exp(x), x.exp());
            assert!(distance <= STD_TOLERANCE_ULPS, "exp({x:e}): {distance} ULP");
        }
    }

    /// Where k changes, the result is built from an `r` at the opposite end of the reduced
    /// range. If `exp` were to go backwards, this is the likeliest place.
    #[test]
    fn no_decrease_across_reduction_boundaries() {
        for k in -1075..=1024 {
            let boundary = (f64::from(k) - 0.5) * core::f64::consts::LN_2;
            let mut below = (boundary - 1e-6).max(LOWEST_NONZERO);
            let mut above = (boundary + 1e-6).min(HIGHEST_FINITE);
            if nearest_multiple_of_ln2(below) == nearest_multiple_of_ln2(above) {
                // The lowest value of k has no boundary below it in range.
                assert_eq!(k, -1075);
                continue;
            }

            // Bisect to the two adjacent floats where k changes.
            while below.next_up() < above {
                let middle = below + (above - below) / 2.0;
                if nearest_multiple_of_ln2(middle) < k {
                    below = middle;
                } else {
                    above = middle;
                }
            }
            assert_eq!(nearest_multiple_of_ln2(below), k - 1);
            assert_eq!(nearest_multiple_of_ln2(above), k);

            let mut x = below;
            for _ in 0..256 {
                x = x.next_down();
            }
            let mut previous = exp(x);
            for _ in 0..512 {
                x = x.next_up();
                let current = exp(x);
                assert!(previous <= current, "exp decreases at {x:e}, k = {k}");
                previous = current;
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20_000))]

        #[test]
        fn close_to_std_over_the_whole_range(x in LOWEST_NONZERO..HIGHEST_FINITE) {
            prop_assert!(ulps(exp(x), x.exp()) <= STD_TOLERANCE_ULPS, "exp({:e})", x);
        }

        /// The range the annealer's acceptance test actually uses.
        #[test]
        fn close_to_std_for_small_inputs(x in -20.0..20.0f64) {
            prop_assert!(ulps(exp(x), x.exp()) <= STD_TOLERANCE_ULPS, "exp({:e})", x);
        }

        #[test]
        fn close_to_std_for_subnormal_results(x in LOWEST_NONZERO..SURELY_SUBNORMAL_BELOW) {
            let result = exp(x);
            prop_assert!(result.is_subnormal(), "exp({:e}) = {:e}", x, result);
            prop_assert!(ulps(result, x.exp()) <= STD_TOLERANCE_ULPS, "exp({:e})", x);
        }

        /// Every bit pattern is a valid input: nothing panics, and the only way to get NaN
        /// is to pass it in.
        #[test]
        fn defined_for_every_input(bits: u64) {
            let x = f64::from_bits(bits);
            let result = exp(x);
            prop_assert_eq!(result.is_nan(), x.is_nan());
            prop_assert!(result.is_nan() || result >= 0.0);
        }

        #[test]
        fn never_decreases_between_arbitrary_inputs(
            a in LOWEST_NONZERO - 1.0..HIGHEST_FINITE + 1.0,
            b in LOWEST_NONZERO - 1.0..HIGHEST_FINITE + 1.0,
        ) {
            let (low, high) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(exp(low) <= exp(high), "{:e}, {:e}", low, high);
        }

        /// The hard case for monotonicity: for inputs this small, one step of the input can
        /// be less than one ULP of the result, so the output moves only through rounding.
        #[test]
        fn never_decreases_between_adjacent_inputs(start in -4.0..4.0f64) {
            let mut x = start;
            let mut previous = exp(x);
            for _ in 0..64 {
                x = x.next_up();
                let current = exp(x);
                prop_assert!(previous <= current, "exp decreases at {:e}", x);
                previous = current;
            }
        }
    }
}
