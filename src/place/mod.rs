//! Generic placement: where to put items so that the nets between them are short.
//!
//! A [`PlacementProblem`] holds items, the nets between their pins, and where items may go:
//! either typed **sites**, discrete slots as on an FPGA, or **free space**, a rectangle on
//! which items' outlines must not overlap, as on a PCB. Items can be fixed, can be limited to
//! a [region](PlacementProblem::add_region), can be allowed some rotations by quarter turns,
//! and can be tied rigidly together into [macros](PlacementProblem::add_macro), as carry chains
//! are. [`check`] says whether a [`Placement`] is legal, without sharing any code with the
//! placers, as the routing validator does for routes.
//!
//! The cost of a placement is a sum over nets, by default the weighted half-perimeter of
//! each net's bounding box, [`Wirelength`]. Other terms join it through [`CostTerm`], such as
//! [`CongestionCost`], which charges for nets that cross areas that a [`CongestionEstimate`]
//! says are congested. [`CongestionGrid`] is one, filled from a placement by RUDY or by a
//! router from what it found.
//!
//! # Placers
//!
//! - [`Annealer`] is simulated annealing after VPR: moves and swaps within a range limit that
//!   shrinks as fewer moves are accepted, or into the median region of an item's nets, a
//!   temperature that falls by how many are, and cost updated incrementally, net by net. It is
//!   a state machine like the routing
//!   [`Engine`](crate::engine::Engine): [`step`](Annealer::step) runs one temperature, and a
//!   run stops at its move budget or a cancel flag with the best placement it has seen.
//!   [`anneal`] runs one from a random legal placement.
//! - [`place_analytic`] is analytic placement after SimPL: a quadratic placement with
//!   bound-to-bound net models, solved by conjugate gradients, alternates with spreading by
//!   recursive bisection, which anchors the next solve. The spread result is then made legal by
//!   [`legalize`], and improved by annealing at a low temperature over short distances.
//!
//! Both are deterministic for a seed: they use no threads, no hash map whose order could
//! matter, and only `f64` operations in a fixed order, with [`crate::math::exp`] for the
//! acceptance test. The `docs/benchmarks.md` file of the repository measures them against
//! problems whose optimum is known, from [`synth`].
//!
//! # Units
//!
//! Coordinates are `i64` and rotations `i32` millidegrees, as elsewhere in the crate. Costs
//! are `u64`. The annealer's range limit is in steps: sites of the item's kind, counted as
//! distinct columns and rows, or multiples of the free space's pitch.

mod abacus;
mod analytic;
mod anneal;
mod check;
mod congestion;
mod cost;
mod eco;
#[cfg(test)]
mod eco_tests;
mod legalize;
mod netlist;
mod problem;
mod spatial;
pub mod synth;
#[cfg(test)]
mod tests;

use core::fmt;

pub use analytic::{AnalyticOptions, AnalyticReport, Inflation, place_analytic};
pub use anneal::{AnnealOptions, Annealer, PlaceOutcome, PlaceStop, TemperatureReport, anneal};
pub use check::{Violation, check};
pub use congestion::{CongestionCost, CongestionEstimate, CongestionGrid};
pub use cost::{CostTerm, NetView, Wirelength};
pub use eco::{
    Displacement, IncrementalOptions, IncrementalOutcome, PlacementChange, Prior, Restricted,
    place_incremental, placement_change, restrict,
};
pub use legalize::legalize;
pub use problem::{
    Fixed, Item, Layout, Pin, Placement, PlacementProblem, ProblemError, Site, rotate,
};

/// A point, or an offset, in layout coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    /// The horizontal coordinate.
    pub x: i64,
    /// The vertical coordinate.
    pub y: i64,
}

impl Point {
    /// Creates a point.
    pub const fn new(x: i64, y: i64) -> Point {
        Point { x, y }
    }

    /// Adds two points as vectors, saturating at the ends of `i64`.
    pub const fn offset(self, by: Point) -> Point {
        Point::new(self.x.saturating_add(by.x), self.y.saturating_add(by.y))
    }

    /// Subtracts `other` as a vector, saturating at the ends of `i64`.
    pub const fn minus(self, other: Point) -> Point {
        Point::new(
            self.x.saturating_sub(other.x),
            self.y.saturating_sub(other.y),
        )
    }

    /// The Manhattan distance to `other`.
    pub const fn manhattan(self, other: Point) -> u64 {
        self.x
            .abs_diff(other.x)
            .saturating_add(self.y.abs_diff(other.y))
    }
}

/// An axis-aligned rectangle, from `min` to `max` inclusive of its edges.
///
/// Two outlines overlap when their interiors do, so outlines that only touch are legal
/// neighbours. A rectangle can be a line or a point, which then overlaps nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    /// The lower left corner.
    pub min: Point,
    /// The upper right corner.
    pub max: Point,
}

impl Rect {
    /// Creates a rectangle from its corners, in any order.
    pub fn new(a: Point, b: Point) -> Rect {
        Rect {
            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    /// Creates a rectangle from its lower left corner and its size.
    pub fn with_size(x: i64, y: i64, width: i64, height: i64) -> Rect {
        Rect::new(Point::new(x, y), Point::new(x + width, y + height))
    }

    /// The rectangle of one point.
    pub const fn point(at: Point) -> Rect {
        Rect { min: at, max: at }
    }

    /// The width, `max.x - min.x`.
    pub const fn width(&self) -> u64 {
        self.max.x.abs_diff(self.min.x)
    }

    /// The height, `max.y - min.y`.
    pub const fn height(&self) -> u64 {
        self.max.y.abs_diff(self.min.y)
    }

    /// The half-perimeter, width plus height.
    pub const fn half_perimeter(&self) -> u64 {
        self.width().saturating_add(self.height())
    }

    /// The area, saturating.
    pub const fn area(&self) -> u64 {
        self.width().saturating_mul(self.height())
    }

    /// Whether `point` is inside or on the edge.
    pub const fn contains(&self, point: Point) -> bool {
        self.min.x <= point.x
            && point.x <= self.max.x
            && self.min.y <= point.y
            && point.y <= self.max.y
    }

    /// Whether `other` lies inside this one, edges included.
    pub const fn contains_rect(&self, other: &Rect) -> bool {
        self.contains(other.min) && self.contains(other.max)
    }

    /// Whether the interiors of the two intersect.
    pub const fn overlaps(&self, other: &Rect) -> bool {
        self.min.x < other.max.x
            && other.min.x < self.max.x
            && self.min.y < other.max.y
            && other.min.y < self.max.y
    }

    /// The rectangle moved by `by`.
    pub const fn translate(&self, by: Point) -> Rect {
        Rect {
            min: self.min.offset(by),
            max: self.max.offset(by),
        }
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            min: Point::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    /// The smallest rectangle holding this one and `point`.
    pub fn including(&self, point: Point) -> Rect {
        self.union(&Rect::point(point))
    }

    /// The overlap of the two, if their interiors or edges meet.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let min = Point::new(self.min.x.max(other.min.x), self.min.y.max(other.min.y));
        let max = Point::new(self.max.x.min(other.max.x), self.max.y.min(other.max.y));
        (min.x <= max.x && min.y <= max.y).then_some(Rect { min, max })
    }

    /// The point inside the rectangle nearest to `point`.
    pub fn clamp(&self, point: Point) -> Point {
        Point::new(
            point.x.clamp(self.min.x, self.max.x),
            point.y.clamp(self.min.y, self.max.y),
        )
    }
}

macro_rules! define_place_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            /// Returns the identifier as an index into a table.
            #[inline]
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

define_place_id! {
    /// Identifies an item of a [`PlacementProblem`].
    ItemId
}

define_place_id! {
    /// Identifies a site of a [`PlacementProblem`] laid out in [sites](Layout::Sites).
    SiteId
}

define_place_id! {
    /// Identifies a macro, a group of items that move together.
    MacroId
}

define_place_id! {
    /// Identifies a region that items can be limited to.
    RegionId
}

/// Why a placer could not run.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaceError {
    /// The problem itself is malformed.
    Problem(ProblemError),
    /// The placement given to start from is not legal. The first violation found is kept.
    IllegalStart(Violation),
    /// No legal place was found for this item, as when there are more items of a kind than
    /// sites, or free space is too crowded for the legalizer.
    NoRoom(ItemId),
}

impl fmt::Display for PlaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlaceError::Problem(problem) => write!(f, "malformed placement problem: {problem}"),
            PlaceError::IllegalStart(violation) => {
                write!(f, "the starting placement is not legal: {violation:?}")
            }
            PlaceError::NoRoom(item) => write!(f, "no legal place found for {item:?}"),
        }
    }
}

impl std::error::Error for PlaceError {}

impl From<ProblemError> for PlaceError {
    fn from(error: ProblemError) -> PlaceError {
        PlaceError::Problem(error)
    }
}
