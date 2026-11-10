//! Routes a design that VPR packed and placed, on a routing resource graph that VPR wrote, and
//! writes the result as a VPR `.route` file.
//!
//! This is the harness for comparing `guide` with VPR on the VTR benchmarks: see
//! `scripts/vtr/` and the FPGA routing section of `docs/benchmarks.md`.
//!
//! ```text
//! cargo run --release --features fpga --example vtr_route -- \
//!     --arch k6_N10_40nm.xml --rr-graph rr.xml --net tseng.net --place tseng.place \
//!     --placement-id SHA256:... --out tseng.route
//! ```
//!
//! Options:
//!
//! | Option | Meaning | Default |
//! | --- | --- | --- |
//! | `--arch FILE` | The architecture, for the delays inside blocks | Required |
//! | `--rr-graph FILE` | The routing resource graph, from VPR's `--write_rr_graph` | Required |
//! | `--net FILE` | The packed netlist | Required |
//! | `--place FILE` | The placement | Required |
//! | `--out FILE` | Where to write the route file, written only if every net is routed legally | None |
//! | `--placement-id ID` | The digest of the placement file for the route file's header, `SHA256:` and its hex | `SHA256:0` |
//! | `--threads N` | Threads to search with | 1 |
//! | `--batch N` | `EngineOptions::batch_size` | 1 |
//! | `--max-iterations N` | `EngineOptions::max_iterations` | The default, 100 |
//! | `--timing-iterations N` | `EngineOptions::timing_iterations` | The default, 0 |
//! | `--history F` | `EngineOptions::history_factor`, in tiles of wire: `RrGraph::delay_normalization` | 1, from `RrGraph::engine_options` |
//! | `--first-present F` | `EngineOptions::first_present` | The default, 0.5 |
//! | `--initial-present F` | `EngineOptions::initial_present` | The default, 0.5 |
//! | `--present-growth F` | `EngineOptions::present_growth` | The default, 1.8 |
//! | `--no-timing` | Route for cost alone | Timing-driven |
//! | `--lookahead KIND` | `map` for `RrGraph::use_map_lookahead`, `location` for the graph's own bound | `map` |
//! | `--lookahead-samples N` | `LookaheadOptions::samples_per_axis` | The default, 2 |
//! | `--astar F` | `EngineOptions::bound_weight` | From `RrGraph::engine_options` |
//! | `--incremental on/off` | `EngineOptions::incremental_reroute` | From `RrGraph::engine_options` |
//! | `--report-timing` | Print the delays that fell back on defaults, and the critical path with each pin's arrival, to standard error | Off |
//!
//! It prints one line of `key=value` pairs, which the scripts parse, and exits with status 0 if
//! the routes are legal, 1 if they are not, and 2 on bad input.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::time::Instant;

use guide::cost::Cost;
use guide::engine::{Outcome, route_parallel, route_timed_parallel};
use guide::fpga::{
    DesignTiming, InternalPin, LookaheadOptions, NetOptions, NodeKind, PackedNetlist, RouteHeader,
    RrGraph, TimingOptions, build_timing, constant_nets, read_architecture, read_netlist,
    read_placement, read_rr_graph, resolve_nets, write_route,
};
use guide::id::NodeId;
use guide::timing::{Constraints, TimingAnalysis};
use guide::validate::check;

/// What the command line asks for.
struct Args {
    arch: String,
    rr_graph: String,
    net: String,
    place: String,
    out: Option<String>,
    placement_id: String,
    threads: usize,
    batch: u32,
    max_iterations: Option<u32>,
    timing_iterations: u32,
    timing: bool,
    /// Factors for the negotiation, `None` for the defaults. History is in tiles of wire.
    history: Option<f64>,
    first_present: Option<f64>,
    initial_present: Option<f64>,
    present_growth: Option<f64>,
    map_lookahead: bool,
    lookahead_samples: Option<u32>,
    astar: Option<f64>,
    incremental: Option<bool>,
    report_timing: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        arch: String::new(),
        rr_graph: String::new(),
        net: String::new(),
        place: String::new(),
        out: None,
        placement_id: "SHA256:0".to_owned(),
        threads: 1,
        batch: 1,
        max_iterations: None,
        timing_iterations: 0,
        timing: true,
        history: None,
        first_present: None,
        initial_present: None,
        present_growth: None,
        map_lookahead: true,
        lookahead_samples: None,
        astar: None,
        incremental: None,
        report_timing: false,
    };
    let mut given = std::env::args().skip(1);
    while let Some(flag) = given.next() {
        if flag == "--no-timing" {
            args.timing = false;
            continue;
        }
        if flag == "--report-timing" {
            args.report_timing = true;
            continue;
        }
        let value = given
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        let number = |value: &str| {
            value
                .parse::<u32>()
                .map_err(|_| format!("{flag} needs a number, not {value:?}"))
        };
        let factor = |value: &str| {
            value
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| format!("{flag} needs a factor, not {value:?}"))
        };
        match flag.as_str() {
            "--arch" => args.arch = value,
            "--rr-graph" => args.rr_graph = value,
            "--net" => args.net = value,
            "--place" => args.place = value,
            "--out" => args.out = Some(value),
            "--placement-id" => args.placement_id = value,
            "--threads" => args.threads = number(&value)? as usize,
            "--batch" => args.batch = number(&value)?,
            "--max-iterations" => args.max_iterations = Some(number(&value)?),
            "--timing-iterations" => args.timing_iterations = number(&value)?,
            "--history" => args.history = Some(factor(&value)?),
            "--first-present" => args.first_present = Some(factor(&value)?),
            "--initial-present" => args.initial_present = Some(factor(&value)?),
            "--present-growth" => args.present_growth = Some(factor(&value)?),
            "--lookahead" => {
                args.map_lookahead = match value.as_str() {
                    "map" => true,
                    "location" => false,
                    _ => return Err(format!("{flag} is map or location, not {value:?}")),
                }
            }
            "--lookahead-samples" => args.lookahead_samples = Some(number(&value)?),
            "--astar" => args.astar = Some(factor(&value)?),
            "--incremental" => {
                args.incremental = Some(match value.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err(format!("{flag} is on or off, not {value:?}")),
                })
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    for (name, value) in [
        ("--arch", &args.arch),
        ("--rr-graph", &args.rr_graph),
        ("--net", &args.net),
        ("--place", &args.place),
    ] {
        if value.is_empty() {
            return Err(format!("{name} is required"));
        }
    }
    Ok(args)
}

fn open(path: &str) -> Result<BufReader<File>, String> {
    File::open(path)
        .map(|file| BufReader::with_capacity(1 << 20, file))
        .map_err(|error| format!("{path}: {error}"))
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

/// Prints what `--report-timing` asks for.
fn report_timing(timing: &DesignTiming, netlist: &PackedNetlist, constraints: &Constraints) {
    for fallback in &timing.report().fallbacks {
        eprintln!("fallback: {fallback:?}");
    }
    let mut analysis = TimingAnalysis::new(timing.graph());
    analysis.run(timing.graph(), constraints);
    let Some(path) = analysis.critical_path() else {
        eprintln!("no critical path");
        return;
    };
    eprintln!(
        "critical path: slack {} ps, arrival {} ps",
        path.slack(),
        path.arrival()
    );
    for &point in path.points() {
        let name = match timing.pin(point) {
            Some(InternalPin::Block(pin)) => format!(
                "block {} {}[{}]",
                netlist.blocks()[pin.block as usize].name,
                netlist.ports()[pin.port as usize],
                pin.bit
            ),
            Some(InternalPin::Primitive { primitive, pin }) => {
                let primitive = &netlist.primitives()[primitive as usize];
                format!(
                    "{} ({}) {}[{}]",
                    primitive.name,
                    primitive.pb_type,
                    netlist.ports()[pin.port as usize],
                    pin.bit
                )
            }
            None => "?".to_owned(),
        };
        eprintln!("  {:>8} {name}", analysis.arrival(point).unwrap_or(0));
    }
}

fn run(args: &Args) -> Result<bool, String> {
    let started = Instant::now();
    let arch = read_architecture(open(&args.arch)?).map_err(|e| format!("{}: {e}", args.arch))?;
    let mut device =
        read_rr_graph(open(&args.rr_graph)?).map_err(|e| format!("{}: {e}", args.rr_graph))?;
    let netlist = read_netlist(open(&args.net)?).map_err(|e| format!("{}: {e}", args.net))?;
    let placement =
        read_placement(open(&args.place)?).map_err(|e| format!("{}: {e}", args.place))?;
    let mut net_options = NetOptions::default();
    net_options.constant_nets = constant_nets(&netlist);
    let constants = net_options.constant_nets.len();
    let design = resolve_nets(&device, &netlist, &placement, &net_options)
        .map_err(|e| format!("resolving nets: {e}"))?;
    let read_seconds = started.elapsed().as_secs_f64();

    let started = Instant::now();
    if args.map_lookahead {
        let mut lookahead = LookaheadOptions::default();
        if let Some(samples) = args.lookahead_samples {
            lookahead.samples_per_axis = samples;
        }
        device.use_map_lookahead(&lookahead);
    }
    let lookahead_seconds = started.elapsed().as_secs_f64();

    let mut options = device.engine_options();
    options.batch_size = args.batch;
    if let Some(max) = args.max_iterations {
        options.max_iterations = max;
    }
    options.timing_iterations = args.timing_iterations;
    let cost = |value: f64| Cost::new(value).ok_or_else(|| format!("bad factor {value}"));
    if let Some(history) = args.history {
        options.history_factor = cost(history * device.delay_normalization())?;
    }
    if let Some(value) = args.first_present {
        options.first_present = cost(value)?;
    }
    if let Some(value) = args.initial_present {
        options.initial_present = cost(value)?;
    }
    if let Some(value) = args.present_growth {
        options.present_growth = cost(value)?;
    }
    if let Some(value) = args.astar {
        options.bound_weight = value;
    }
    if let Some(value) = args.incremental {
        options.incremental_reroute = value;
    }

    let timed = device.timed_graph();
    let started = Instant::now();
    let outcome = if args.timing {
        let mut timing = build_timing(&netlist, arch.delays(), &TimingOptions::default())
            .map_err(|e| format!("timing graph: {e}"))?;
        timing.estimate_delays(&timed, &design);
        let constraints = timing
            .vpr_constraints(0)
            .map_err(|e| format!("constraints: {e}"))?;
        let driven = timing.timing_driven(&design, constraints.clone());
        let connections = driven.connections.clone();
        let outcome = route_timed_parallel(&timed, design.nets(), options, driven, args.threads);
        if args.report_timing
            && let Ok(outcome) = &outcome
            && let Some(result) = &outcome.timing
        {
            timing.set_connection_delays(&connections, &result.connection_delays);
            report_timing(&timing, &netlist, &constraints);
        }
        outcome
    } else {
        route_parallel(device.graph(), design.nets(), options, args.threads)
    }
    .map_err(|e| format!("engine: {e}"))?;
    let route_seconds = started.elapsed().as_secs_f64();

    let report = check(device.graph(), design.nets(), &outcome.routes);
    let legal = outcome.is_success() && report.is_complete() && report.faults().is_empty();
    let critical = outcome
        .timing
        .as_ref()
        .and_then(|timing| timing.metrics.critical_path_delay);
    let slack = outcome
        .timing
        .as_ref()
        .and_then(|timing| timing.metrics.worst_slack);

    let mut written = false;
    if legal && let Some(path) = &args.out {
        let file = File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let mut out = BufWriter::new(file);
        let header = RouteHeader {
            placement_file: &args.place,
            placement_id: &args.placement_id,
        };
        write_route(&mut out, &device, &design, &outcome.routes, &header)
            .map_err(|e| format!("{path}: {e}"))?;
        out.flush().map_err(|e| format!("{path}: {e}"))?;
        written = true;
    }

    println!(
        "legal={legal} validator_faults={} overused={} iterations={} best_iteration={} \
         stop={:?} nets={} constant_nets={constants} wirelength={} critical_path_ps={} worst_slack_ps={} total_cost={:.6e} \
         expansions={} read_s={read_seconds:.3} lookahead_s={lookahead_seconds:.3} \
         route_s={route_seconds:.3} threads={} \
         batch={} written={written}",
        report.faults().len(),
        outcome.overused.len(),
        outcome.routes.stats.iterations,
        outcome.best_iteration,
        outcome.stop,
        design.nets().len(),
        wirelength(&device, &outcome),
        critical.map_or_else(|| "none".to_owned(), |ps| ps.to_string()),
        slack.map_or_else(|| "none".to_owned(), |ps| ps.to_string()),
        outcome.total_cost.get(),
        outcome.routes.stats.nodes_expanded,
        args.threads,
        args.batch,
    );
    Ok(legal)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("vtr_route: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("vtr_route: {message}");
            ExitCode::from(2)
        }
    }
}
