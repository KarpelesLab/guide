//! # guide
//!
//! A placement and routing engine for connection problems that share the same
//! core algorithms: printed circuit boards, FPGAs, and anything else that comes
//! down to connecting terminals through contended resources.
//!
//! **The API is not stable yet**, and changes between 0.0.x releases. See the [roadmap] for
//! what is done and what is next, and the [architecture notes] for the design.
//!
//! The core needs no feature and has no dependency. A [`RoutingSpace`](space::RoutingSpace)
//! says where routes can go and what they compete for; [`engine`] routes nets through one by
//! negotiated congestion, [`search`] finds single paths, [`steiner`] builds trees, [`timing`]
//! analyses delays, and [`validate`] checks routes without sharing any code with the router.
//! [`space`] has ready-made spaces, grids and graphs:
//!
//! ```
//! use guide::engine::{EngineOptions, route};
//! use guide::id::NetId;
//! use guide::net::{Net, Terminal};
//! use guide::space::grid::Grid;
//! use guide::validate;
//!
//! // Two nets that cross on a grid of two layers: one of them changes layer.
//! let grid = Grid::new(5, 5, 2)?;
//! let at = |x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
//! let nets = [
//!     Net { id: NetId(0), terminals: vec![at(0, 2), at(4, 2)] },
//!     Net { id: NetId(1), terminals: vec![at(2, 0), at(2, 4)] },
//! ];
//! let outcome = route(&grid, &nets, EngineOptions::default())?;
//! let routes = outcome.legal_routes().expect("this problem can be routed");
//! assert!(validate::check(&grid, &nets, routes).is_legal());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [`place`] places items on sites or in free space, by simulated annealing or analytic
//! placement, and needs no feature. [`eco`] routes and places again after a small change to a
//! problem, keeping what did not change. Results are deterministic: the same inputs, seed and
//! options give the same result, for any thread count and on any platform.
//!
//! The `fpga` feature adds support for FPGA routing problems, and the `pcb` feature adds
//! the geometry layer, `geom`, and PCB routing over it, `pcb`. The `kicad` feature, which
//! implies `pcb`, reads KiCad boards into them, for clearance checks and routing, and the `dsn`
//! feature, which implies `pcb` too, reads Specctra DSN designs and writes SES sessions. The
//! `mapf` feature adds multi-agent path finding, `mapf`, routed by the same
//! engine through a time-expanded space, and the `asic` feature ASIC global routing, `asic`, on
//! the ISPD contests' designs.
//!
//! The `cli` feature builds the `guide` command-line tool, with every domain: `cargo install
//! guide --features cli`. A library user needs none of it.
//!
//! [roadmap]: https://github.com/KarpelesLab/guide/blob/master/ROADMAP.md
//! [architecture notes]: https://github.com/KarpelesLab/guide/blob/master/docs/architecture.md

// On docs.rs, which builds with `--cfg docsrs`, mark each item with the feature it needs.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "asic")]
pub mod asic;
pub mod cost;
#[cfg(feature = "dsn")]
pub mod dsn;
pub mod eco;
pub mod engine;
#[cfg(feature = "fpga")]
pub mod fpga;
#[cfg(feature = "pcb")]
pub mod geom;
pub mod id;
#[cfg(feature = "kicad")]
pub mod kicad;
#[cfg(feature = "mapf")]
pub mod mapf;
pub mod math;
pub mod net;
#[cfg(feature = "pcb")]
pub mod pcb;
pub mod place;
pub mod rng;
pub mod search;
#[cfg(any(feature = "kicad", feature = "dsn"))]
mod sexpr;
pub mod space;
pub mod steiner;
pub mod timing;
#[cfg(any(feature = "kicad", feature = "dsn"))]
mod units;
pub mod validate;
