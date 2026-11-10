//! What a placement costs.

use super::{Point, Rect};
use crate::id::NetId;

/// One net, as a cost term sees it.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct NetView<'a> {
    /// The net.
    pub net: NetId,
    /// Its weight.
    pub weight: u32,
    /// Where its pins are, in the order they were given, so the first is the driver if the
    /// problem's nets list it first.
    pub pins: &'a [Point],
    /// The bounding box of the pins.
    pub bounds: Rect,
}

/// A cost per net, summed over nets to give a placement's cost.
///
/// Placers keep each net's cost and only ask again for nets whose pins moved, so a term has
/// to depend on nothing but the net and what it is given here. A term that reads something
/// else, such as a [congestion estimate](super::CongestionEstimate), must keep that fixed for
/// the length of a run.
///
/// Terms combine as a pair, `(A, B)`, whose cost is the sum of the two.
///
/// # Examples
///
/// A timing-like term that charges each sink's distance from the driver:
///
/// ```
/// use guide::place::{CostTerm, NetView, Wirelength};
///
/// struct DriverDistance;
///
/// impl CostTerm for DriverDistance {
///     fn net_cost(&self, net: &NetView<'_>) -> u64 {
///         let Some((driver, sinks)) = net.pins.split_first() else { return 0 };
///         sinks.iter().map(|&sink| driver.manhattan(sink)).sum()
///     }
/// }
///
/// let both = (Wirelength, DriverDistance);
/// # let _ = both;
/// ```
pub trait CostTerm {
    /// The cost of one net.
    fn net_cost(&self, net: &NetView<'_>) -> u64;
}

/// Weighted half-perimeter wirelength: a net's weight times the width plus the height of its
/// pins' bounding box. This is the usual placement cost, and the one [`PlacementProblem`]
/// computes in [`wirelength`](super::PlacementProblem::wirelength).
///
/// [`PlacementProblem`]: super::PlacementProblem
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Wirelength;

impl CostTerm for Wirelength {
    #[inline]
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        net.bounds
            .half_perimeter()
            .saturating_mul(u64::from(net.weight))
    }
}

impl<A: CostTerm, B: CostTerm> CostTerm for (A, B) {
    #[inline]
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        self.0.net_cost(net).saturating_add(self.1.net_cost(net))
    }
}

impl<T: CostTerm + ?Sized> CostTerm for &T {
    #[inline]
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        (**self).net_cost(net)
    }
}
