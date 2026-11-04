//! # guide
//!
//! A placement and routing engine for connection problems that share the same
//! core algorithms: printed circuit boards, FPGAs, and anything else that comes
//! down to connecting terminals through contended resources.
//!
//! **This crate is at an early stage and is not usable for routing yet.** See the
//! [roadmap] for the plan and the [architecture notes] for the intended design.
//!
//! [roadmap]: https://github.com/KarpelesLab/guide/blob/master/ROADMAP.md
//! [architecture notes]: https://github.com/KarpelesLab/guide/blob/master/docs/architecture.md

pub mod cost;
pub mod engine;
pub mod id;
pub mod math;
pub mod net;
pub mod rng;
pub mod search;
pub mod space;
pub mod steiner;
pub mod validate;
