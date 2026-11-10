//! ASIC global routing, on the problems of the ISPD 2007 and 2008 global routing contests.
//!
//! | What | Done by | Becomes |
//! | --- | --- | --- |
//! | A design (`.gr`) | [`read_design`] | [`Design`] |
//! | Routing it | [`route_design`] | [`AsicRouting`], with each net's wires and vias as [`Segment`]s |
//! | Writing routes | [`write_net_route`] | The contests' route format |
//! | Reading routes, from here or any other router | [`read_routes`] | [`NetRoute`]s |
//! | Scoring them | [`evaluate`] | [`Evaluation`]: overflow and wirelength as `eval2008.pl` has them |
//!
//! # The flow
//!
//! As most of the contests' routers do, nets are routed on one plane first, and given layers
//! after:
//!
//! 1. **The plane.** Every layer's capacity on an edge is turned into tracks, its capacity over
//!    the width and spacing of a wire, and the tracks of all layers are added up, into a
//!    [`PlanarSpace`]. The [engine](crate::engine) routes every net there with negotiated
//!    congestion: each net's tree grows from its first pin to the nearest pin not yet
//!    connected, by A\*, which is the Steiner heuristic of Takahashi and Matsuyama, and nets
//!    on edges that overflow are ripped up and routed again, with history, until none do.
//! 2. **Layers.** Each net's tree is then given layers, net by net, shortest first, exactly for
//!    the fewest vias that don't overflow a layer, by dynamic programming over the tree, as in
//!    COLA. Nets on edges that still overflow are given layers again, with those edges dearer.
//!
//! Nets with more than 1000 pins, and nets whose pins are all in one cell, are not routed, as
//! the contests allow. Pins are where the file has them, on their layers, and routes reach them
//! there through vias.
//!
//! # Example
//!
//! ```
//! use guide::asic::{AsicOptions, NetRoute, evaluate, read_design, read_routes, route_design, write_net_route};
//!
//! let text = "grid 3 3 2
//! vertical capacity 0 2
//! horizontal capacity 2 0
//! minimum width 1 1
//! minimum spacing 1 1
//! via spacing 1 1
//! 0 0 10 10
//! num net 2
//! A 0 2 1
//!  5  5 1
//! 25 25 1
//! B 1 2 1
//!  5 25 1
//! 25  5 1
//! ";
//! let design = read_design(text.as_bytes())?;
//! let routing = route_design(&design, &AsicOptions::default());
//!
//! let mut file = Vec::new();
//! for (net, segments) in design.nets.iter().zip(&routing.routes) {
//!     write_net_route(&mut file, &design, net, segments)?;
//! }
//! let routes = read_routes(file.as_slice())?;
//! let score = evaluate(&design, &routes, 1);
//! assert!(score.is_complete());
//! assert_eq!(score.total_overflow, 0);
//! // Each net crosses four edges. With one track on each layer they can't both take an L
//! // shape, and every turn takes a via, as does every pin on a wire of layer 2.
//! assert_eq!((score.wire_length, score.vias), (8, 6));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod eval;
mod format;
mod layers;
mod pattern;
mod planar;
mod router;
#[cfg(test)]
mod tests;

pub use eval::{Evaluation, RouteFault, evaluate};
pub use format::{
    Adjustment, Capacities, Design, DesignNet, EdgeAt, Gcell, NetRoute, Pin, Point3, ReadError,
    Segment, read_design, read_routes, write_net_route,
};
pub use planar::PlanarSpace;
pub use router::{AsicOptions, AsicRouting, MAX_PINS, route_design};
