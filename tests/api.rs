//! Guarantees of the public API that the compiler can check, so that losing one is a
//! failing test rather than a surprise for a user: every error type is a
//! `std::error::Error + Send + Sync + 'static`, and so can go into a `Box<dyn Error + Send +
//! Sync>` or across threads; the main data, problem and result types are `Send + Sync`; and
//! every options struct has `Default`, `Clone` and `Debug`. `docs/api-review.md` explains
//! the choices, and `docs/stability.md` what a release may change.
//!
//! Nothing runs: each check is a function that only compiles if the bound holds.

#![allow(dead_code)]

use std::error::Error;

fn error<E: Error + Send + Sync + 'static>() {}
fn shared<T: Send + Sync>() {}
fn options<T: Default + Clone + core::fmt::Debug>() {}

#[test]
fn core_errors() {
    use guide::*;
    error::<engine::EngineError>();
    error::<net::EmptyTerminal>();
    error::<net::PathError<u32>>();
    error::<net::RouteError<u32>>();
    error::<place::PlaceError>();
    error::<place::ProblemError>();
    error::<space::graph::GraphError>();
    error::<space::grid::GridError>();
    error::<steiner::SteinerError>();
    error::<steiner::RefineError<u32>>();
    error::<timing::TimingError>();
    error::<validate::Fault<u32, u32>>();
}

#[test]
fn core_types_are_send_and_sync() {
    use guide::*;
    shared::<cost::Cost>();
    shared::<net::Net<id::NodeId>>();
    shared::<net::RouteSet<id::NodeId>>();
    shared::<space::grid::Grid>();
    shared::<space::graph::Graph>();
    shared::<search::Search<space::grid::Grid>>();
    shared::<engine::Outcome<id::NodeId, id::NodeId>>();
    shared::<engine::EngineOptions<'static>>();
    shared::<validate::Report<id::NodeId, id::NodeId>>();
    shared::<steiner::SteinerTree<id::NodeId>>();
    shared::<timing::TimingGraph>();
    shared::<timing::TimingAnalysis>();
    shared::<place::PlacementProblem>();
    shared::<place::Placement>();
    shared::<place::AnnealOptions<'static>>();
    shared::<place::PlaceOutcome>();
    shared::<place::Annealer<'static, place::Wirelength>>();
    shared::<rng::Rng>();
}

#[test]
fn state_machines_are_debug() {
    use guide::*;
    fn debug<T: core::fmt::Debug>() {}
    debug::<engine::Engine<'static, space::grid::Grid>>();
    debug::<place::Annealer<'static, place::Wirelength>>();
    debug::<search::Search<space::grid::Grid>>();
}

#[test]
fn core_options() {
    use guide::*;
    options::<engine::EngineOptions<'static>>();
    options::<search::SearchLimits<'static>>();
    options::<steiner::SteinerOptions>();
    options::<timing::CriticalityOptions>();
    options::<place::AnnealOptions<'static>>();
    options::<place::AnalyticOptions<'static>>();
    options::<place::IncrementalOptions>();
}

#[cfg(feature = "fpga")]
#[test]
fn fpga() {
    use guide::fpga::*;
    error::<ImportError>();
    error::<LoopError>();
    error::<RouteFileError>();
    error::<XmlErrorKind>();
    shared::<RrGraph>();
    shared::<PackedNetlist>();
    shared::<Architecture>();
    shared::<Design>();
    shared::<Placement>();
    shared::<PlaceProblem>();
    options::<LoopOptions>();
    options::<LookaheadOptions>();
    options::<NetOptions>();
    options::<PlacerOptions>();
    options::<ProblemOptions>();
    options::<RefineOptions>();
    options::<TimingOptions>();
}

#[cfg(feature = "pcb")]
#[test]
fn pcb() {
    use guide::geom::{self, drc};
    use guide::pcb::{self, place};
    error::<geom::GeomError>();
    error::<geom::RuleError>();
    error::<drc::DrcError>();
    error::<pcb::RouteError>();
    shared::<geom::Shape>();
    shared::<geom::Rules>();
    shared::<geom::ShapeIndex>();
    shared::<drc::Board>();
    shared::<pcb::Design>();
    shared::<pcb::Routed>();
    shared::<place::PlacementDesign>();
    options::<pcb::RouteOptions>();
    options::<place::PlaceOptions>();
}

#[cfg(feature = "kicad")]
#[test]
fn kicad() {
    use guide::kicad::*;
    error::<KicadError>();
    error::<FlowError>();
    shared::<KicadBoard>();
    shared::<FlowReport>();
    options::<FlowOptions>();
}

#[cfg(feature = "dsn")]
#[test]
fn dsn() {
    use guide::dsn::*;
    error::<DsnError>();
    shared::<DsnBoard>();
}

#[cfg(feature = "mapf")]
#[test]
fn mapf() {
    use guide::mapf::*;
    error::<GraphError>();
    error::<InstanceError>();
    error::<check::Fault>();
    error::<movingai::ParseError>();
    shared::<Graph>();
    shared::<Instance<'static>>();
    shared::<Solution>();
    options::<SolveOptions<'static>>();
}

#[cfg(feature = "asic")]
#[test]
fn asic() {
    use guide::asic::*;
    error::<ReadError>();
    shared::<Design>();
    shared::<AsicRouting>();
    shared::<Evaluation>();
    options::<AsicOptions>();
}
