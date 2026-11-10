//! # guide
//!
//! A placement and routing engine for connection problems that share the same
//! core algorithms: printed circuit boards, FPGAs, and anything else that comes
//! down to connecting terminals through contended resources.
//!
//! **This crate is at an early stage and is not usable for routing yet.** See the
//! [roadmap] for the plan and the [architecture notes] for the intended design.
//!
//! The `fpga` feature adds support for FPGA routing problems, and the `pcb` feature adds
//! the geometry layer, `geom`, and PCB routing over it, `pcb`. The `kicad` feature, which
//! implies `pcb`, reads KiCad boards into them, for clearance checks and routing, and the `dsn`
//! feature, which implies `pcb` too, reads Specctra DSN designs and writes SES sessions.
//!
//! [roadmap]: https://github.com/KarpelesLab/guide/blob/master/ROADMAP.md
//! [architecture notes]: https://github.com/KarpelesLab/guide/blob/master/docs/architecture.md

pub mod cost;
#[cfg(feature = "dsn")]
pub mod dsn;
pub mod engine;
#[cfg(feature = "fpga")]
pub mod fpga;
#[cfg(feature = "pcb")]
pub mod geom;
pub mod id;
#[cfg(feature = "kicad")]
pub mod kicad;
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
