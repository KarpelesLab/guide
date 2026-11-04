//! Routing costs.

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::iter::Sum;
use core::ops::{Add, Mul};

/// A routing cost: a finite, non-negative `f64`.
///
/// The invariant has three consequences that the engines rely on:
///
/// - Costs are totally ordered, so `Cost` is [`Ord`] and [`Eq`].
/// - Negative zero is normalized away, which makes [`Cost::key`] monotonic: comparing
///   keys is the same as comparing costs, in one integer instruction.
/// - Arithmetic cannot produce NaN. Results too large to represent saturate at
///   [`Cost::MAX`], so a cost can never become infinite.
///
/// # Examples
///
/// ```
/// use guide::cost::Cost;
///
/// let a = Cost::new(1.5).unwrap();
/// let b = Cost::from(2u32);
/// assert_eq!((a + b).get(), 3.5);
/// assert!(a.key() < b.key());
///
/// assert_eq!(Cost::new(-1.0), None);
/// assert_eq!(Cost::new(f64::NAN), None);
/// ```
#[derive(Clone, Copy, Default)]
pub struct Cost(f64);

impl Cost {
    /// No cost.
    pub const ZERO: Cost = Cost(0.0);
    /// A cost of one.
    pub const ONE: Cost = Cost(1.0);
    /// The largest representable cost, where arithmetic saturates.
    pub const MAX: Cost = Cost(f64::MAX);

    /// Creates a cost, or returns `None` if `value` is negative, infinite or NaN.
    ///
    /// Negative zero is accepted and becomes positive zero.
    #[inline]
    pub fn new(value: f64) -> Option<Cost> {
        if value.is_finite() && value >= 0.0 {
            // In the default rounding mode, -0.0 + 0.0 is +0.0.
            Some(Cost(value + 0.0))
        } else {
            None
        }
    }

    /// Returns the value as an `f64`.
    #[inline]
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Returns an integer key that orders exactly as the costs do.
    ///
    /// Priority queues can compare these instead of floating-point values.
    #[inline]
    pub const fn key(self) -> u64 {
        self.0.to_bits()
    }

    #[inline]
    fn saturate(value: f64) -> Cost {
        if value.is_finite() {
            Cost(value)
        } else {
            Cost::MAX
        }
    }
}

impl From<u32> for Cost {
    /// Converts exactly: every `u32` is representable as an `f64`.
    #[inline]
    fn from(value: u32) -> Cost {
        Cost(f64::from(value))
    }
}

impl PartialEq for Cost {
    #[inline]
    fn eq(&self, other: &Cost) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Cost {}

impl PartialOrd for Cost {
    #[inline]
    fn partial_cmp(&self, other: &Cost) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cost {
    #[inline]
    fn cmp(&self, other: &Cost) -> Ordering {
        self.key().cmp(&other.key())
    }
}

impl Hash for Cost {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl Add for Cost {
    type Output = Cost;

    /// Adds two costs, saturating at [`Cost::MAX`].
    #[inline]
    fn add(self, other: Cost) -> Cost {
        Cost::saturate(self.0 + other.0)
    }
}

impl Mul for Cost {
    type Output = Cost;

    /// Multiplies two costs, saturating at [`Cost::MAX`].
    #[inline]
    fn mul(self, other: Cost) -> Cost {
        Cost::saturate(self.0 * other.0)
    }
}

impl Sum for Cost {
    /// Adds costs in iteration order, which fixes the rounding.
    fn sum<I: Iterator<Item = Cost>>(iter: I) -> Cost {
        iter.fold(Cost::ZERO, Add::add)
    }
}

impl fmt::Debug for Cost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for Cost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rejects_values_outside_the_invariant() {
        assert_eq!(Cost::new(-1.0), None);
        assert_eq!(Cost::new(-f64::MIN_POSITIVE), None);
        assert_eq!(Cost::new(f64::INFINITY), None);
        assert_eq!(Cost::new(f64::NAN), None);
    }

    #[test]
    fn normalizes_negative_zero() {
        let zero = Cost::new(-0.0).unwrap();
        assert_eq!(zero.key(), Cost::ZERO.key());
        assert!(zero.get().is_sign_positive());
    }

    #[test]
    fn arithmetic_saturates_instead_of_overflowing() {
        assert_eq!(Cost::MAX + Cost::MAX, Cost::MAX);
        assert_eq!(Cost::MAX * Cost::from(2), Cost::MAX);
        assert_eq!(Cost::MAX * Cost::ZERO, Cost::ZERO);
    }

    #[test]
    fn sum_of_nothing_is_zero() {
        assert_eq!(core::iter::empty::<Cost>().sum::<Cost>(), Cost::ZERO);
    }

    fn cost() -> impl Strategy<Value = Cost> {
        (0.0..=f64::MAX).prop_map(|v| Cost::new(v).unwrap())
    }

    proptest! {
        #[test]
        fn key_orders_like_the_value(a in cost(), b in cost()) {
            prop_assert_eq!(a.key().cmp(&b.key()), a.get().total_cmp(&b.get()));
            prop_assert_eq!(a.cmp(&b), a.get().total_cmp(&b.get()));
        }

        #[test]
        fn arithmetic_upholds_the_invariant(a in cost(), b in cost()) {
            prop_assert!(Cost::new((a + b).get()).is_some());
            prop_assert!(Cost::new((a * b).get()).is_some());
        }
    }
}
