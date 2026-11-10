//! Congestion feedback: routing estimates that placement can price.
//!
//! [`CongestionEstimate`] is the narrow interface by which placement learns where routing is
//! short of room. Anything can implement it: a map filled by a global router from the
//! resources it found overused, or [`CongestionGrid::rudy`], which needs no router at all.
//! [`CongestionCost`] turns an estimate into a [`CostTerm`] that makes nets dearer when their
//! bounding box crosses congested areas, and pairs with [`Wirelength`](super::Wirelength).
//!
//! An estimate is read during a placer's run, and has to stay fixed for its length, since
//! placers keep each net's cost and only ask again for nets that move. Feeding a new estimate
//! back means starting another run from the result, which is the loop of the roadmap's M9.

use super::cost::{CostTerm, NetView};
use super::{Placement, PlacementProblem, Point, Rect};
use crate::id::NetId;

/// Where routing is expected to be congested.
pub trait CongestionEstimate {
    /// How congested `area` is, as the average over it of how far demand exceeds capacity, in
    /// thousandths of capacity. Zero means there is room everywhere in it.
    fn overflow(&self, area: &Rect) -> u64;
}

impl<T: CongestionEstimate + ?Sized> CongestionEstimate for &T {
    fn overflow(&self, area: &Rect) -> u64 {
        (**self).overflow(area)
    }
}

/// Demand for routing and capacity for it, on a grid of square bins.
///
/// Fill it from a placement with [`rudy`](CongestionGrid::rudy), or from a router with
/// [`add_demand_at`](CongestionGrid::add_demand_at) and
/// [`set_capacity_at`](CongestionGrid::set_capacity_at), for example from the use of a global
/// router's tiles.
///
/// Demand and capacity are both in units of wire length per bin, `f64`s added in a fixed
/// order, so a grid filled the same way is the same everywhere.
#[derive(Clone, Debug, PartialEq)]
pub struct CongestionGrid {
    origin: Point,
    bin: i64,
    nx: usize,
    ny: usize,
    demand: Vec<f64>,
    capacity: Vec<f64>,
}

impl CongestionGrid {
    /// A grid over `area` with square bins `bin` wide, each with `capacity` and no demand.
    ///
    /// # Panics
    ///
    /// If `bin` is below 1.
    pub fn new(area: Rect, bin: i64, capacity: f64) -> CongestionGrid {
        assert!(bin >= 1, "bins must be at least 1 wide");
        let nx = (area.width() as i64 / bin + 1) as usize;
        let ny = (area.height() as i64 / bin + 1) as usize;
        CongestionGrid {
            origin: area.min,
            bin,
            nx,
            ny,
            demand: vec![0.0; nx * ny],
            capacity: vec![capacity; nx * ny],
        }
    }

    /// The RUDY estimate of a placement (Spindler and Johannes, 2007): each net spreads a
    /// demand of its weight times its half-perimeter evenly over its bounding box, as if its
    /// wire were smeared across it. Nets whose box is a line or a point are widened to one
    /// unit, so that they still count.
    ///
    /// `capacity` is per bin, in the same units: for example, the tracks a bin holds times its
    /// width.
    pub fn rudy(
        problem: &PlacementProblem,
        placement: &Placement,
        bin: i64,
        capacity: f64,
    ) -> CongestionGrid {
        let mut grid = CongestionGrid::new(problem.extent(), bin, capacity);
        for net in 0..problem.net_count() {
            let net = NetId(net as u32);
            if problem.net_pins(net).len() < 2 {
                continue;
            }
            if let Some(bounds) = problem.net_box(placement, net) {
                let wire = bounds.half_perimeter() as f64 * f64::from(problem.net_weight(net));
                grid.spread(&bounds, wire);
            }
        }
        grid
    }

    /// Spreads `wire` of demand evenly over `area`, in proportion to how much of each bin it
    /// covers.
    pub fn spread(&mut self, area: &Rect, wire: f64) {
        let area = Rect {
            min: area.min,
            max: Point::new(
                area.max.x.max(area.min.x + 1),
                area.max.y.max(area.min.y + 1),
            ),
        };
        let total = area.width() as f64 * area.height() as f64;
        let (x0, x1, y0, y1) = self.span(&area);
        for x in x0..=x1 {
            for y in y0..=y1 {
                let bin = self.bin_rect(x, y);
                if let Some(overlap) = bin.intersection(&area) {
                    let share = overlap.width() as f64 * overlap.height() as f64 / total;
                    self.demand[x * self.ny + y] += wire * share;
                }
            }
        }
    }

    /// Adds demand to the bin holding `at`.
    pub fn add_demand_at(&mut self, at: Point, demand: f64) {
        let b = self.bin_of(at);
        self.demand[b] += demand;
    }

    /// Sets the capacity of the bin holding `at`.
    pub fn set_capacity_at(&mut self, at: Point, capacity: f64) {
        let b = self.bin_of(at);
        self.capacity[b] = capacity;
    }

    /// The demand on the bin holding `at`.
    pub fn demand_at(&self, at: Point) -> f64 {
        self.demand[self.bin_of(at)]
    }

    /// Demand over capacity on the bin holding `at`, or infinity for a bin with demand and no
    /// capacity.
    pub fn utilization_at(&self, at: Point) -> f64 {
        let b = self.bin_of(at);
        ratio(self.demand[b], self.capacity[b])
    }

    /// The number of bins across and up.
    pub fn size(&self) -> (usize, usize) {
        (self.nx, self.ny)
    }

    /// The highest utilization of any bin.
    pub fn peak_utilization(&self) -> f64 {
        self.demand
            .iter()
            .zip(&self.capacity)
            .map(|(&d, &c)| ratio(d, c))
            .fold(0.0, f64::max)
    }

    fn bin_rect(&self, x: usize, y: usize) -> Rect {
        Rect::with_size(
            self.origin.x + x as i64 * self.bin,
            self.origin.y + y as i64 * self.bin,
            self.bin,
            self.bin,
        )
    }

    fn bin_of(&self, at: Point) -> usize {
        let (x, _, y, _) = self.span(&Rect::point(at));
        x * self.ny + y
    }

    fn span(&self, rect: &Rect) -> (usize, usize, usize, usize) {
        let cx = |v: i64| {
            (v - self.origin.x)
                .div_euclid(self.bin)
                .clamp(0, self.nx as i64 - 1) as usize
        };
        let cy = |v: i64| {
            (v - self.origin.y)
                .div_euclid(self.bin)
                .clamp(0, self.ny as i64 - 1) as usize
        };
        (
            cx(rect.min.x),
            cx(rect.max.x),
            cy(rect.min.y),
            cy(rect.max.y),
        )
    }
}

fn ratio(demand: f64, capacity: f64) -> f64 {
    if demand <= 0.0 {
        0.0
    } else if capacity <= 0.0 {
        f64::INFINITY
    } else {
        demand / capacity
    }
}

impl CongestionEstimate for CongestionGrid {
    fn overflow(&self, area: &Rect) -> u64 {
        let (x0, x1, y0, y1) = self.span(area);
        let mut sum = 0.0;
        for x in x0..=x1 {
            for y in y0..=y1 {
                let b = x * self.ny + y;
                // A bin with no capacity counts as ten times full, rather than infinitely.
                let over = (ratio(self.demand[b], self.capacity[b]).min(10.0) - 1.0).max(0.0);
                sum += over;
            }
        }
        let bins = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f64;
        (sum / bins * 1000.0) as u64
    }
}

/// A cost term for congestion: a net's weighted half-perimeter, times how far the bins its
/// bounding box covers are over capacity, times a factor.
///
/// With `factor_permille` at 1000, a net whose box is on average twice as full as it may be
/// (an overflow of 1000) costs its wirelength again on top of [`Wirelength`]. Pair the two as
/// `(Wirelength, CongestionCost { .. })`.
///
/// [`Wirelength`]: super::Wirelength
#[derive(Clone, Debug)]
pub struct CongestionCost<E> {
    /// Where routing is congested.
    pub estimate: E,
    /// How much congestion weighs against wirelength, in thousandths.
    pub factor_permille: u64,
}

impl<E: CongestionEstimate> CostTerm for CongestionCost<E> {
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        if net.pins.len() < 2 {
            return 0;
        }
        let overflow = self.estimate.overflow(&net.bounds);
        if overflow == 0 {
            return 0;
        }
        let wire = u128::from(net.bounds.half_perimeter()) * u128::from(net.weight);
        let extra = wire * u128::from(overflow) * u128::from(self.factor_permille) / 1_000_000;
        u64::try_from(extra).unwrap_or(u64::MAX)
    }
}
