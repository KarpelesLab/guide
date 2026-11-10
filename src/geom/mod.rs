//! Geometry for PCB-like domains: shapes, exact clearances, a spatial index, design rules
//! and a design rule check. Enabled by the `pcb` feature.
//!
//! Built on [polyclip](https://crates.io/crates/polyclip), which none of this exposes: cargo
//! treats each polyclip 0.0.x release as breaking, so a polyclip type in this API would
//! force `guide` and its users to upgrade together. A private module is the only place that
//! names it, and a test keeps it that way.
//!
//! # Coordinates
//!
//! [`Point`]s are `i64` database units, nanometres on a PCB, with Y up as in polyclip and
//! cadlab. Coordinates are limited to ±[`MAX_COORD`], 2^40, which is polyclip's limit.
//! Everything that takes geometry checks that, and returns [`GeomError`] for anything out
//! of range rather than panicking or computing a wrong answer.
//!
//! # Exactness
//!
//! A [`Shape`] is all the points within a radius of a core: a point for a circle, a
//! segment for a track, a polygon for a pad. Round things are never turned into polygons,
//! so circles, tracks, rectangles, rounded rectangles and polygons are exact, and
//! [`Shape::violates`], [`Shape::touches`] and [`Shape::distance`] give exact answers for
//! them. A gap equal to the clearance is not a violation, and touching is a short.
//!
//! The answer comes from the first of these that can give it: the cores' bounding boxes in
//! integers, then a filter in `f64` with error bounds wide enough that it only answers when
//! it is sure, then polyclip's exact distance comparison in `i128` and wider, and lastly,
//! for radii ending in a half, an exact rational comparison here. The filter decides most
//! pairs of tracks and vias, which have no inside, and only says "too close" for polygons.
//!
//! **Arcs are the exception.** polyclip approximates them by polylines, so an
//! [arc track](Shape::arc_track) is a polyline within a tolerance `t` of the true
//! centerline, which is [`DEFAULT_ARC_TOLERANCE`], 100 nm, unless chosen, plus a unit for
//! rounding vertices to integers. Every distance to an arc is reduced by `t + 1`, so that
//! it is a lower bound on the true distance. The answer can be wrong in one direction only:
//! a gap between the clearance and the clearance plus `2t + 2` can be reported as a
//! violation that isn't one. A real violation is never missed. [`Shape::is_exact`] and
//! [`Gap::is_exact`] say when this applies. To make such a false report less likely,
//! lower the tolerance, which costs vertices: a full circle of 1 mm radius takes 195 at
//! 100 nm, and 615 at 10 nm.
//!
//! # The index
//!
//! [`SpatialIndex`] is a dynamic R-tree over rectangles, and [`ShapeIndex`] adds shapes
//! and clearance queries to it. Both return entries sorted by [`EntryId`], so that results
//! depend only on the operations done, never on hashing or the tree's shape.
//!
//! The R-tree was chosen over a uniform grid by `benches/geom.rs`, on a synthetic board of
//! 100 × 80 mm with fine-pitch ICs, passives, vias, tracks of every length, and a few
//! large pours and keepouts. A grid with the best cell size for that board inserted ten
//! times as fast, churned about as fast, and queried 15% slower. With cells four times too
//! small it was three to ten times slower at everything. One cell size can't suit every
//! board, and a grid needs the board's extent too, while the R-tree needs neither and was
//! never more than 15% behind the best grid, apart from one-at-a-time insertion, which
//! [`SpatialIndex::bulk_load`] makes up for. So the grid lives on only in the benchmark,
//! whose documentation has the numbers.
//!
//! # Rules and checks
//!
//! [`RuleSpec`] holds the design rules as plain data, and [`Rules`] validates them and
//! answers what applies where, by the [precedence](Rules#precedence) it documents.
//! [`drc::check`] checks tracks, vias, pads, zones and keepouts against them: clearances
//! between nets, with those of pairs of nets and of items of their own, shorts, hole
//! clearances and holes too close, widths, drills and rings. It reports every violation in
//! a fixed order, and shares no code with any router. It is exact by default, where KiCad
//! lets a clearance fall 500 nm short: [`clearance_slack`](drc::Board::clearance_slack) set
//! to [`KICAD_CLEARANCE_SLACK`](drc::KICAD_CLEARANCE_SLACK) decides as KiCad does. With the
//! `kicad` feature, `kicad::read` reads KiCad's boards for it, and on nine real boards it
//! agrees with KiCad's own check, but for differences that are each explained in
//! `docs/benchmarks.md`.

#![deny(private_interfaces, private_bounds)]

mod clearance;
pub mod drc;
mod index;
mod poly;
mod rules;
mod shape;
mod types;

pub use clearance::Gap;
pub use index::{EntryId, ShapeIndex, SpatialIndex};
pub(crate) use poly::{ArcSide, Area, fractured};
pub use rules::{
    ClassId, LayerId, LayerSet, MAX_LAYERS, NetClass, Overrides, RuleError, RuleSpec, Rules,
    ViaDef, ViaId,
};
pub use shape::{DEFAULT_ARC_TOLERANCE, Shape};
pub use types::{GeomError, MAX_COORD, Point, Rect};

#[cfg(test)]
mod tests;
