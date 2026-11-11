//! `guide fpga`: VPR's files in, VPR's files out.

use std::io::BufWriter;

use guide::engine::{EngineOptions, Outcome, StopReason, route_parallel, route_timed_parallel};
use guide::fpga::{
    Architecture, Design, LookaheadOptions, LoopOptions, NetOptions, NodeKind, PackedNetlist,
    PlaceDelayModel, PlaceMethod, PlaceProblem, PlacerOptions, ProblemOptions, RouteHeader,
    RrGraph, TimingOptions, build_place_problem, build_timing, constant_nets, estimate_slack,
    place_design, place_route_loop, read_architecture, read_netlist, read_placement, read_rr_graph,
    resolve_nets, write_placement, write_route,
};
use guide::id::NodeId;
use guide::validate::check;

use crate::Outcome as CommandOutcome;
use crate::args::{self, Command, Failure, Matches, Opt};
use crate::report::{Deadline, Map, Ui, open, sha256_hex, write_output};

const ARCH: Opt = Opt::value(
    "arch",
    "FILE",
    "VPR's architecture, arch.xml, for the delays inside blocks (not needed with --no-timing for routing)",
);
const RR_GRAPH: Opt = Opt::value(
    "rr-graph",
    "FILE",
    "The routing resource graph, from VPR's --write_rr_graph",
);
const NET: Opt = Opt::value("net", "FILE", "The packed netlist, .net");
const NO_TIMING: Opt = Opt::flag("no-timing", "Optimise wire alone, not timing");
const BATCH: Opt = Opt::value(
    "batch",
    "N",
    "Nets routed at once against the same congestion, which changes results; threads help only above 1 [default: 1]",
);
const MAX_ITERATIONS: Opt = Opt::value(
    "max-iterations",
    "N",
    "Iterations of negotiated congestion at most",
);
const WRITE_ILLEGAL: Opt = Opt::flag(
    "write-illegal",
    "Write the route file even if the routing is not legal (VPR will refuse to load it)",
);

fn route_command() -> Command {
    Command {
        name: "fpga route",
        about: "Routes a design that VPR packed and placed, on a routing resource graph that VPR \
                wrote, and writes VPR's .route file.",
        operands: "--arch FILE --rr-graph FILE --net FILE --place FILE",
        count: (0, 0),
        options: vec![
            ARCH,
            RR_GRAPH,
            NET,
            Opt::value("place", "FILE", "The placement, .place"),
            args::output("Where to write the .route file, if the routing is legal"),
            Opt::value(
                "placement-id",
                "ID",
                "The placement's ID for the header [default: SHA256: and the digest of --place]",
            ),
            NO_TIMING,
            Opt::value(
                "lookahead",
                "KIND",
                "map, VPR's map lookahead, or location, the graph's own bound [default: map]",
            ),
            BATCH,
            MAX_ITERATIONS,
            Opt::value(
                "max-expansions",
                "N",
                "Search nodes expanded at most: a budget that stops at the same point every run",
            ),
            WRITE_ILLEGAL,
            Opt::value(
                "seed",
                "N",
                "Shuffle the nets between iterations with this seed [default: no shuffle]",
            ),
            args::THREADS,
            args::TIME_LIMIT,
            args::JSON,
            args::QUIET,
            args::VERBOSE,
        ],
        details: "Example:\n  guide fpga route --arch k6_N10_40nm.xml --rr-graph rr.xml --net c.net \
                  --place c.place -o c.route\n\n\
                  The route file is what VPR's --route_file and --analysis load. --rr-graph, --net \
                  and --place are required.",
    }
}

fn place_command() -> Command {
    Command {
        name: "fpga place",
        about: "Places a design that VPR packed, on the device of a routing resource graph that \
                VPR wrote, and writes VPR's .place file.",
        operands: "--arch FILE --rr-graph FILE --net FILE",
        count: (0, 0),
        options: vec![
            ARCH,
            RR_GRAPH,
            NET,
            args::output("Where to write the .place file, if the placement is legal"),
            Opt::value(
                "netlist-id",
                "ID",
                "The netlist's ID for the header [default: SHA256: and the digest of --net]",
            ),
            Opt::value(
                "fix",
                "FILE",
                "A placement whose blocks stay where it puts them, as VPR's --fix_clusters",
            ),
            Opt::value(
                "method",
                "KIND",
                "analytic, or anneal for simulated annealing alone [default: analytic]",
            ),
            Opt::value(
                "effort",
                "F",
                "Moves per temperature of the annealing, as VPR's --inner_num [default: 1]",
            ),
            NO_TIMING,
            Opt::value("seed", "N", "The placer's seed [default: 1]"),
            args::JSON,
            args::QUIET,
            args::VERBOSE,
        ],
        details: "Example:\n  guide fpga place --arch k6_N10_40nm.xml --rr-graph rr.xml --net c.net \
                  -o c.place\n\n--arch, --rr-graph and --net are required. Placement has no time \
                  limit.",
    }
}

fn flow_command() -> Command {
    Command {
        name: "fpga flow",
        about: "Places a design that VPR packed, routes it, and where routing fails, places again \
                around what it overused and routes again, a few rounds. Writes VPR's .place and \
                .route files.",
        operands: "--arch FILE --rr-graph FILE --net FILE",
        count: (0, 0),
        options: vec![
            ARCH,
            RR_GRAPH,
            NET,
            Opt::value(
                "route-rr-graph",
                "FILE",
                "Another graph of the same device to route on, such as one of a narrower channel \
                 [default: --rr-graph]",
            ),
            args::output("Where to write the .place file"),
            Opt::value(
                "route-output",
                "FILE",
                "Where to write the .route file, if the routing is legal",
            ),
            Opt::value(
                "rounds",
                "N",
                "Times to place again after a routing that fails [default: 3]",
            ),
            Opt::value("method", "KIND", "The first placement: analytic or anneal"),
            NO_TIMING,
            BATCH,
            MAX_ITERATIONS,
            WRITE_ILLEGAL,
            Opt::value("seed", "N", "The placer's seed [default: 1]"),
            args::THREADS,
            args::TIME_LIMIT,
            args::JSON,
            args::QUIET,
            args::VERBOSE,
        ],
        details: "Example:\n  guide fpga flow --arch k6_N10_40nm.xml --rr-graph rr.xml --net c.net \
                  -o c.place --route-output c.route\n\n\
                  --arch, --rr-graph and --net are required. The time limit covers routing: \
                  placing is not stopped by it.",
    }
}

/// The wirelength as VPR counts it: the tiles spanned by every wire that a route uses.
fn wirelength(device: &RrGraph, outcome: &Outcome<NodeId, NodeId>) -> u64 {
    let mut total = 0;
    for (_, route) in outcome.routes.routes() {
        for node in route.nodes() {
            if matches!(
                device.node_kind(node),
                Some(NodeKind::ChanX | NodeKind::ChanY)
            ) && let Some(at) = device.node_location(node)
            {
                total += u64::from(at.xhigh - at.xlow) + u64::from(at.yhigh - at.ylow) + 1;
            }
        }
    }
    total
}

fn read_arch(path: &str) -> Result<Architecture, Failure> {
    read_architecture(open(path)?).map_err(|e| Failure::file(path, e))
}

fn read_device(path: &str) -> Result<RrGraph, Failure> {
    read_rr_graph(open(path)?).map_err(|e| Failure::file(path, e))
}

fn read_net(path: &str) -> Result<PackedNetlist, Failure> {
    read_netlist(open(path)?).map_err(|e| Failure::file(path, e))
}

/// `SHA256:` and the digest of a file, as VPR's headers have it.
fn digest(path: &str) -> Result<String, Failure> {
    let bytes = std::fs::read(path).map_err(|e| Failure::file(path, e))?;
    Ok(format!("SHA256:{}", sha256_hex(&bytes)))
}

fn stop_name(stop: StopReason) -> String {
    format!("{stop:?}")
}

/// What a routing came to, for the report.
fn routing_report(
    report: &mut Map,
    device: &RrGraph,
    design: &Design,
    outcome: &Outcome<NodeId, NodeId>,
    deadline: &Deadline,
) -> bool {
    let faults = check(device.graph(), design.nets(), &outcome.routes);
    let legal = outcome.is_success() && faults.is_complete() && faults.faults().is_empty();
    let timing = outcome.timing.as_ref().map(|t| t.metrics);
    report
        .set("legal", legal)
        .set("nets", design.nets().len())
        .set("overused", outcome.overused.len())
        .set("validator_faults", faults.faults().len())
        .set("iterations", outcome.routes.stats.iterations)
        .set("stop", stop_name(outcome.stop))
        .set("time_limit_hit", deadline.expired())
        .set("wirelength", wirelength(device, outcome))
        .set(
            "critical_path_ps",
            timing.and_then(|t| t.critical_path_delay),
        )
        .set("worst_slack_ps", timing.and_then(|t| t.worst_slack))
        .set("expansions", outcome.routes.stats.nodes_expanded);
    legal
}

/// Applies the routing options common to `route` and `flow`, and `--seed` as the seed that
/// shuffles nets if `shuffle` says.
fn engine_options<'c>(
    m: &Matches,
    device: &RrGraph,
    deadline: &'c Deadline,
    shuffle: bool,
) -> Result<EngineOptions<'c>, Failure> {
    let mut options = device.engine_options();
    if let Some(batch) = m.number("batch")? {
        options.batch_size = batch;
    }
    if let Some(max) = m.number("max-iterations")? {
        options.max_iterations = max;
    }
    if m.get("max-expansions").is_some() {
        options.max_expansions = m.number("max-expansions")?.unwrap_or(u64::MAX);
    }
    if shuffle {
        options.shuffle_seed = m.number("seed")?;
    }
    options.cancel = deadline.flag();
    Ok(options)
}

/// `guide fpga route`.
pub fn route(argv: &[String]) -> CommandOutcome {
    let command = route_command();
    let m = command.parse(argv)?;
    let ui = Ui::new(&m);
    let timing_driven = !m.flag("no-timing");
    let rr_path = m.required("rr-graph")?;
    let net_path = m.required("net")?;
    let place_path = m.required("place")?;
    let arch_path = if timing_driven {
        Some(m.required("arch")?)
    } else {
        m.get("arch")
    };
    let map_lookahead = m.choice("lookahead", &["map", "location"])? != Some(1);
    let threads = m.threads()?;
    let limit = m.seconds("time-limit")?;

    ui.progress(format!("reading {rr_path}"));
    let mut device = read_device(rr_path)?;
    let netlist = read_net(net_path)?;
    let placement = read_placement(open(place_path)?).map_err(|e| Failure::file(place_path, e))?;
    let arch = arch_path.map(read_arch).transpose()?;
    let mut net_options = NetOptions::default();
    net_options.constant_nets = constant_nets(&netlist);
    let design = resolve_nets(&device, &netlist, &placement, &net_options)
        .map_err(|e| Failure::Input(format!("{place_path}: resolving nets: {e}")))?;
    ui.detail(format!(
        "{} nets, {} left unrouted as constants",
        design.nets().len(),
        net_options.constant_nets.len()
    ));
    if map_lookahead {
        ui.progress("building the map lookahead");
        device.use_map_lookahead(&LookaheadOptions::default());
    }

    let deadline = Deadline::start(limit);
    let options = engine_options(&m, &device, &deadline, true)?;
    ui.progress(if timing_driven {
        "routing, timing-driven"
    } else {
        "routing"
    });
    let outcome = match (&arch, timing_driven) {
        (Some(arch), true) => {
            let mut timing = build_timing(&netlist, arch.delays(), &TimingOptions::default())
                .map_err(|e| Failure::Input(format!("{net_path}: timing graph: {e}")))?;
            let timed = device.timed_graph();
            timing.estimate_delays(&timed, &design);
            let constraints = timing
                .vpr_constraints(0)
                .map_err(|e| Failure::Input(format!("timing constraints: {e}")))?;
            let driven = timing.timing_driven(&design, constraints);
            route_timed_parallel(&timed, design.nets(), options, driven, threads)
        }
        _ => route_parallel(device.graph(), design.nets(), options, threads),
    }
    .map_err(|e| Failure::Input(format!("the engine refused the nets: {e}")))?;

    let mut report = Map::default();
    let legal = routing_report(&mut report, &device, &design, &outcome, &deadline);
    let mut written = None;
    if let Some(path) = m.get("output")
        && (legal || m.flag("write-illegal"))
    {
        let id = match m.get("placement-id") {
            Some(id) => id.to_owned(),
            None => digest(place_path)?,
        };
        let mut text = Vec::new();
        write_route(
            &mut text,
            &device,
            &design,
            &outcome.routes,
            &RouteHeader {
                placement_file: place_path,
                placement_id: &id,
            },
        )
        .map_err(|e| Failure::file(path, e))?;
        write_output(path, &text)?;
        written = Some(path);
    }
    report.set("written", written);
    ui.finish("fpga route", report)?;
    Ok(legal)
}

/// The placement problem of a design.
fn problem(
    device: &RrGraph,
    netlist: &PackedNetlist,
    arch: &Architecture,
    fix: Option<&str>,
) -> Result<PlaceProblem, Failure> {
    let fixed = match fix {
        Some(path) => Some(read_placement(open(path)?).map_err(|e| Failure::file(path, e))?),
        None => None,
    };
    let mut options = ProblemOptions::default();
    options.fixed = fixed.as_ref();
    options.ignored_nets = constant_nets(netlist);
    build_place_problem(device, netlist, arch, &options)
        .map_err(|e| Failure::Input(format!("building the placement problem: {e}")))
}

fn placer_options(m: &Matches) -> Result<PlacerOptions, Failure> {
    let mut options = PlacerOptions::default();
    if let Some(seed) = m.number("seed")? {
        options.seed = seed;
    }
    if let Some(method) = m.choice("method", &["analytic", "anneal"])? {
        options.method = [PlaceMethod::Analytic, PlaceMethod::Anneal][method];
    }
    if let Some(effort) = m.factor("effort")? {
        options.inner_num = effort;
    }
    Ok(options)
}

/// Writes a placement, with the netlist's name and ID in its header.
fn write_place(
    path: &str,
    problem: &PlaceProblem,
    netlist: &PackedNetlist,
    placement: &guide::place::Placement,
    net_path: &str,
    net_id: &str,
) -> Result<Vec<u8>, Failure> {
    let placement = problem.to_placement(netlist, placement, net_path, net_id);
    let mut text = BufWriter::new(Vec::new());
    write_placement(&mut text, &placement).map_err(|e| Failure::file(path, e))?;
    let text = text
        .into_inner()
        .map_err(|e| Failure::file(path, e.error()))?;
    write_output(path, &text)?;
    Ok(text)
}

/// `guide fpga place`.
pub fn place(argv: &[String]) -> CommandOutcome {
    let command = place_command();
    let m = command.parse(argv)?;
    let ui = Ui::new(&m);
    let (arch_path, rr_path, net_path) = (
        m.required("arch")?,
        m.required("rr-graph")?,
        m.required("net")?,
    );
    let options = placer_options(&m)?;

    ui.progress(format!("reading {rr_path}"));
    let arch = read_arch(arch_path)?;
    let device = read_device(rr_path)?;
    let netlist = read_net(net_path)?;
    let problem = problem(&device, &netlist, &arch, m.get("fix"))?;
    let delays = PlaceDelayModel::from_rr_graph(&device);
    drop(device);
    let mut timing = build_timing(&netlist, arch.delays(), &TimingOptions::default())
        .map_err(|e| Failure::Input(format!("{net_path}: timing graph: {e}")))?;
    let constraints = timing
        .vpr_constraints(0)
        .map_err(|e| Failure::Input(format!("timing constraints: {e}")))?;

    ui.progress("placing");
    let timing_arg = (!m.flag("no-timing")).then_some((&mut timing, &constraints));
    let result = place_design(&problem, &delays, timing_arg, &options)
        .map_err(|e| Failure::Input(format!("placing: {e}")))?;
    let violations = guide::place::check(problem.problem(), &result.placement);
    let legal = violations.is_empty();
    let slack = estimate_slack(
        &problem,
        &delays,
        &mut timing,
        &constraints,
        &result.placement,
    );
    let mut report = Map::default();
    report
        .set("legal", legal)
        .set("violations", violations.len())
        .set("blocks", netlist.blocks().len())
        .set("nets", problem.problem().net_count())
        .set("bb_estimate", result.bb_estimate)
        .set("start_bb_estimate", result.start_bb_estimate)
        .set("worst_slack_ps", slack)
        .set("moves", result.moves);
    let mut written = None;
    if legal && let Some(path) = m.get("output") {
        let id = match m.get("netlist-id") {
            Some(id) => id.to_owned(),
            None => digest(net_path)?,
        };
        write_place(path, &problem, &netlist, &result.placement, net_path, &id)?;
        written = Some(path);
    }
    report.set("written", written);
    ui.finish("fpga place", report)?;
    Ok(legal)
}

/// `guide fpga flow`.
pub fn flow(argv: &[String]) -> CommandOutcome {
    let command = flow_command();
    let m = command.parse(argv)?;
    let ui = Ui::new(&m);
    let (arch_path, rr_path, net_path) = (
        m.required("arch")?,
        m.required("rr-graph")?,
        m.required("net")?,
    );
    let timing_driven = !m.flag("no-timing");
    let placer = placer_options(&m)?;
    let threads = m.threads()?;
    let limit = m.seconds("time-limit")?;
    let mut loop_options = LoopOptions::default();
    if let Some(rounds) = m.number("rounds")? {
        loop_options.rounds = rounds;
    }
    loop_options.threads = threads;
    loop_options.refine.seed = placer.seed;
    if !timing_driven {
        loop_options.refine.timing_tradeoff = 0.0;
    }

    ui.progress(format!("reading {rr_path}"));
    let arch = read_arch(arch_path)?;
    let device = read_device(rr_path)?;
    let netlist = read_net(net_path)?;
    let problem = problem(&device, &netlist, &arch, None)?;
    let delays = PlaceDelayModel::from_rr_graph(&device);
    let mut route_device = match m.get("route-rr-graph") {
        Some(path) => {
            drop(device);
            read_device(path)?
        }
        None => device,
    };
    let mut timing = build_timing(&netlist, arch.delays(), &TimingOptions::default())
        .map_err(|e| Failure::Input(format!("{net_path}: timing graph: {e}")))?;
    let constraints = timing
        .vpr_constraints(0)
        .map_err(|e| Failure::Input(format!("timing constraints: {e}")))?;

    ui.progress("placing");
    let start = place_design(
        &problem,
        &delays,
        timing_driven.then_some((&mut timing, &constraints)),
        &placer,
    )
    .map_err(|e| Failure::Input(format!("placing: {e}")))?;
    ui.progress("building the map lookahead");
    route_device.use_map_lookahead(&LookaheadOptions::default());
    let deadline = Deadline::start(limit);
    let mut engine = engine_options(&m, &route_device, &deadline, false)?;
    if m.get("max-iterations").is_none() {
        engine.max_iterations = 50;
    }
    let mut net_options = NetOptions::default();
    net_options.constant_nets = constant_nets(&netlist);
    ui.progress("routing, and placing again where it fails");
    let result = place_route_loop(
        &route_device,
        &netlist,
        &problem,
        &delays,
        timing_driven.then_some((&mut timing, &constraints)),
        &net_options,
        engine,
        start.placement,
        &loop_options,
    )
    .map_err(|e| Failure::Input(e.to_string()))?;
    for (i, round) in result.rounds.iter().enumerate() {
        ui.detail(format!(
            "round {i}: legal={} overused={} iterations={} moved={}",
            round.legal, round.overused, round.iterations, round.moved
        ));
    }

    let mut report = Map::default();
    let legal_routes = routing_report(
        &mut report,
        &route_device,
        &result.design,
        &result.outcome,
        &deadline,
    );
    let placed_legal = guide::place::check(problem.problem(), &result.placement).is_empty();
    let legal = legal_routes && placed_legal;
    report
        .set("placement_legal", placed_legal)
        .set("rounds", result.rounds.len())
        .set("bb_estimate", problem.bb_estimate(&result.placement));
    let mut written = Vec::new();
    let mut place_text = None;
    if placed_legal && let Some(path) = m.get("output") {
        let id = digest(net_path)?;
        place_text = Some(write_place(
            path,
            &problem,
            &netlist,
            &result.placement,
            net_path,
            &id,
        )?);
        written.push(path.to_owned());
    }
    if let Some(path) = m.get("route-output")
        && (legal || m.flag("write-illegal"))
    {
        let (file, id) = match (m.get("output"), &place_text) {
            (Some(file), Some(text)) => (file, format!("SHA256:{}", sha256_hex(text))),
            _ => ("unwritten.place", "SHA256:0".to_owned()),
        };
        let mut text = Vec::new();
        write_route(
            &mut text,
            &route_device,
            &result.design,
            &result.outcome.routes,
            &RouteHeader {
                placement_file: file,
                placement_id: &id,
            },
        )
        .map_err(|e| Failure::file(path, e))?;
        write_output(path, &text)?;
        written.push(path.to_owned());
    }
    report.set("written", written);
    ui.finish("fpga flow", report)?;
    Ok(legal)
}
