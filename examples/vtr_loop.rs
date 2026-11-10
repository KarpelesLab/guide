//! Places and routes a design that VPR packed in a loop at one channel width: from a
//! placement, routes it, and where that fails places again around what the router overused,
//! with what it found added to placement's estimate of congestion, and routes again. Writes
//! the placement the loop ends with as a VPR `.place` file.
//!
//! This is the harness for measuring the loop against the one-pass flow on the VTR
//! benchmarks: see `scripts/vtr/loop-circuit.sh` and the section on the loop in
//! `docs/benchmarks.md`.
//!
//! ```text
//! cargo run --release --features fpga --example vtr_loop -- \
//!     --arch arch.xml --rr-graph w76/rr.xml --route-rr-graph w60/rr.xml --net c.net \
//!     --place guide.place --out loop.place --netlist-file c.net --netlist-id SHA256:...
//! ```
//!
//! Options:
//!
//! | Option | Meaning | Default |
//! | --- | --- | --- |
//! | `--arch FILE` | The architecture | Required |
//! | `--rr-graph FILE` | The graph the placement problem is built on, as for `vtr_place` | Required |
//! | `--route-rr-graph FILE` | The graph of the channel width to route at | Required |
//! | `--net FILE` | The packed netlist | Required |
//! | `--place FILE` | The placement to start from | Required |
//! | `--out FILE` | Where to write the placement the loop ends with, if it routed | None |
//! | `--netlist-file NAME` | The netlist's file name for the placement's header | The `--net` path |
//! | `--netlist-id ID` | The digest of the netlist file for the header | `SHA256:0` |
//! | `--rounds N` | `LoopOptions::rounds` | 3 |
//! | `--margin N` | `LoopOptions::margin` | 6 |
//! | `--whole` | `LoopOptions::whole` | Off |
//! | `--seed-routes` | `LoopOptions::seed_routes` | Off |
//! | `--gap F` | `LoopOptions::gap_weight` | 1 |
//! | `--history F` | `LoopOptions::history_weight` | 1 |
//! | `--seed N` | `RefineOptions::seed` | 1 |
//! | `--inner-num F` | `RefineOptions::inner_num` | 1 |
//! | `--temperature F` | `RefineOptions::temperature` | 0.05 |
//! | `--range N` | `RefineOptions::range` | 4 |
//! | `--congestion-factor F` | `RefineOptions::congestion_factor` | 1 |
//! | `--congestion-threshold F` | `RefineOptions::congestion_threshold` | 0.6 |
//! | `--max-iterations N` | `EngineOptions::max_iterations` of each routing | 50 |
//! | `--threads N` | Threads to route with | 1 |
//! | `--no-timing` | Place and route for wire alone | Timing-driven |
//!
//! It prints one line of `key=value` pairs, then one per round, and exits with status 0 if the
//! placement it ends with routed, 1 if none did, and 2 on bad input.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::time::Instant;

use guide::fpga::{
    LookaheadOptions, LoopOptions, NetOptions, PlaceDelayModel, ProblemOptions, TimingOptions,
    build_place_problem, build_timing, constant_nets, place_route_loop, read_architecture,
    read_netlist, read_placement, read_rr_graph, write_placement,
};
use guide::place::check;

/// What the command line asks for.
struct Args {
    arch: String,
    rr_graph: String,
    route_rr_graph: String,
    net: String,
    place: String,
    out: Option<String>,
    netlist_file: Option<String>,
    netlist_id: String,
    max_iterations: u32,
    timing: bool,
    options: LoopOptions,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        arch: String::new(),
        rr_graph: String::new(),
        route_rr_graph: String::new(),
        net: String::new(),
        place: String::new(),
        out: None,
        netlist_file: None,
        netlist_id: "SHA256:0".to_owned(),
        max_iterations: 50,
        timing: true,
        options: LoopOptions::default(),
    };
    let mut given = std::env::args().skip(1);
    while let Some(flag) = given.next() {
        match flag.as_str() {
            "--no-timing" => {
                args.timing = false;
                continue;
            }
            "--whole" => {
                args.options.whole = true;
                continue;
            }
            "--seed-routes" => {
                args.options.seed_routes = true;
                continue;
            }
            _ => {}
        }
        let value = given
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        let number = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{flag} needs a number, not {value:?}"))
        };
        let factor = |value: &str| {
            value
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| format!("{flag} needs a factor, not {value:?}"))
        };
        let refine = &mut args.options.refine;
        match flag.as_str() {
            "--arch" => args.arch = value,
            "--rr-graph" => args.rr_graph = value,
            "--route-rr-graph" => args.route_rr_graph = value,
            "--net" => args.net = value,
            "--place" => args.place = value,
            "--out" => args.out = Some(value),
            "--netlist-file" => args.netlist_file = Some(value),
            "--netlist-id" => args.netlist_id = value,
            "--rounds" => args.options.rounds = number(&value)? as u32,
            "--margin" => args.options.margin = number(&value)? as u32,
            "--gap" => args.options.gap_weight = factor(&value)?,
            "--history" => args.options.history_weight = factor(&value)?,
            "--seed" => refine.seed = number(&value)?,
            "--inner-num" => refine.inner_num = factor(&value)?,
            "--temperature" => refine.temperature = factor(&value)?,
            "--range" => refine.range = number(&value)? as u32,
            "--congestion-factor" => refine.congestion_factor = factor(&value)?,
            "--congestion-threshold" => refine.congestion_threshold = factor(&value)?,
            "--max-iterations" => args.max_iterations = number(&value)? as u32,
            "--threads" => args.options.threads = number(&value)? as usize,
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    for (name, value) in [
        ("--arch", &args.arch),
        ("--rr-graph", &args.rr_graph),
        ("--route-rr-graph", &args.route_rr_graph),
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

fn run(args: &Args) -> Result<bool, String> {
    let started = Instant::now();
    let arch = read_architecture(open(&args.arch)?).map_err(|e| format!("{}: {e}", args.arch))?;
    let device =
        read_rr_graph(open(&args.rr_graph)?).map_err(|e| format!("{}: {e}", args.rr_graph))?;
    let netlist = read_netlist(open(&args.net)?).map_err(|e| format!("{}: {e}", args.net))?;
    let start = read_placement(open(&args.place)?).map_err(|e| format!("{}: {e}", args.place))?;
    let mut problem_options = ProblemOptions::default();
    problem_options.ignored_nets = constant_nets(&netlist);
    let problem = build_place_problem(&device, &netlist, &arch, &problem_options)
        .map_err(|e| format!("building the problem: {e}"))?;
    let delays = PlaceDelayModel::from_rr_graph(&device);
    drop(device);
    let start = problem
        .from_placement(&netlist, &start)
        .map_err(|e| format!("{}: {e}", args.place))?;
    let mut route_device = read_rr_graph(open(&args.route_rr_graph)?)
        .map_err(|e| format!("{}: {e}", args.route_rr_graph))?;
    route_device.use_map_lookahead(&LookaheadOptions::default());
    let mut net_options = NetOptions::default();
    net_options.constant_nets = constant_nets(&netlist);
    let mut timing = build_timing(&netlist, arch.delays(), &TimingOptions::default())
        .map_err(|e| format!("timing graph: {e}"))?;
    let constraints = timing
        .vpr_constraints(0)
        .map_err(|e| format!("constraints: {e}"))?;
    let read_seconds = started.elapsed().as_secs_f64();

    let mut engine_options = route_device.engine_options();
    engine_options.max_iterations = args.max_iterations;
    let mut options = args.options;
    if !args.timing {
        options.refine.timing_tradeoff = 0.0;
    }
    let started = Instant::now();
    let result = place_route_loop(
        &route_device,
        &netlist,
        &problem,
        &delays,
        args.timing.then_some((&mut timing, &constraints)),
        &net_options,
        engine_options,
        start,
        &options,
    )
    .map_err(|e| e.to_string())?;
    let loop_seconds = started.elapsed().as_secs_f64();

    let legal =
        result.outcome.is_success() && check(problem.problem(), &result.placement).is_empty();
    let mut written = false;
    if legal && let Some(path) = &args.out {
        let file_name = args.netlist_file.as_deref().unwrap_or(&args.net);
        let placement =
            problem.to_placement(&netlist, &result.placement, file_name, &args.netlist_id);
        let file = File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let mut out = BufWriter::new(file);
        write_placement(&mut out, &placement).map_err(|e| format!("{path}: {e}"))?;
        out.flush().map_err(|e| format!("{path}: {e}"))?;
        written = true;
    }
    println!(
        "legal={legal} rounds={} bb_estimate={:.0} read_s={read_seconds:.3} \
         loop_s={loop_seconds:.3} written={written}",
        result.rounds.len(),
        problem.bb_estimate(&result.placement),
    );
    for (index, round) in result.rounds.iter().enumerate() {
        println!(
            "round={index} legal={} overused={} iterations={} bb_estimate={:.0} \
             worst_slack_ps={} windows={} window_tiles={} moved={}",
            round.legal,
            round.overused,
            round.iterations,
            round.bb_estimate,
            round
                .worst_slack
                .map_or_else(|| "none".to_owned(), |s| s.to_string()),
            round.windows,
            round.window_tiles,
            round.moved,
        );
    }
    Ok(legal)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("vtr_loop: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("vtr_loop: {message}");
            ExitCode::from(2)
        }
    }
}
