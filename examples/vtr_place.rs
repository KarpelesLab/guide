//! Places a design that VPR packed, on the device of a routing resource graph that VPR wrote,
//! and writes the result as a VPR `.place` file.
//!
//! This is the harness for comparing `guide`'s placement with VPR's on the VTR benchmarks:
//! see `scripts/vtr/` and the FPGA placement section of `docs/benchmarks.md`.
//!
//! ```text
//! cargo run --release --features fpga --example vtr_place -- \
//!     --arch k6_N10_40nm.xml --rr-graph rr.xml --net tseng.net --out tseng.place \
//!     --netlist-file tseng.net --netlist-id SHA256:...
//! ```
//!
//! Options:
//!
//! | Option | Meaning | Default |
//! | --- | --- | --- |
//! | `--arch FILE` | The architecture, for the delays inside blocks and the carry chains | Required |
//! | `--rr-graph FILE` | A routing resource graph of the device, from VPR's `--write_rr_graph` | Required |
//! | `--net FILE` | The packed netlist | Required |
//! | `--out FILE` | Where to write the placement | None |
//! | `--netlist-file NAME` | The netlist's file name for the placement's header | The `--net` path |
//! | `--netlist-id ID` | The digest of the netlist file for the header, `SHA256:` and its hex | `SHA256:0` |
//! | `--fix FILE` | A placement whose blocks stay where it puts them, as VPR's `--fix_clusters` | None |
//! | `--evaluate FILE` | Another placement, such as VPR's, to measure as this one is | None |
//! | `--method KIND` | `analytic` or `anneal` | `analytic` |
//! | `--seed N` | `PlacerOptions::seed` | 1 |
//! | `--inner-num F` | `PlacerOptions::inner_num` | 1 |
//! | `--tradeoff F` | `PlacerOptions::timing_tradeoff` | 0.5 |
//! | `--detailed-temperature F` | `PlacerOptions::detailed_temperature` | 0.05 |
//! | `--detailed-range N` | `PlacerOptions::detailed_range` | 8 |
//! | `--no-timing` | Place for wirelength alone | Timing-driven |
//!
//! It prints one line of `key=value` pairs, which the scripts parse, and exits with status 0 if
//! the placement is legal, 1 if it is not, and 2 on bad input.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::time::Instant;

use guide::fpga::{
    PlaceDelayModel, PlaceMethod, PlacerOptions, ProblemOptions, TimingOptions,
    build_place_problem, build_timing, constant_nets, estimate_slack, place_design,
    read_architecture, read_netlist, read_placement, read_rr_graph, write_placement,
};
use guide::place::check;

/// What the command line asks for.
struct Args {
    arch: String,
    rr_graph: String,
    net: String,
    out: Option<String>,
    netlist_file: Option<String>,
    netlist_id: String,
    fix: Option<String>,
    evaluate: Option<String>,
    timing: bool,
    options: PlacerOptions,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        arch: String::new(),
        rr_graph: String::new(),
        net: String::new(),
        out: None,
        netlist_file: None,
        netlist_id: "SHA256:0".to_owned(),
        fix: None,
        evaluate: None,
        timing: true,
        options: PlacerOptions::default(),
    };
    let mut given = std::env::args().skip(1);
    while let Some(flag) = given.next() {
        if flag == "--no-timing" {
            args.timing = false;
            continue;
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
        match flag.as_str() {
            "--arch" => args.arch = value,
            "--rr-graph" => args.rr_graph = value,
            "--net" => args.net = value,
            "--out" => args.out = Some(value),
            "--netlist-file" => args.netlist_file = Some(value),
            "--netlist-id" => args.netlist_id = value,
            "--fix" => args.fix = Some(value),
            "--evaluate" => args.evaluate = Some(value),
            "--method" => {
                args.options.method = match value.as_str() {
                    "anneal" => PlaceMethod::Anneal,
                    "analytic" => PlaceMethod::Analytic,
                    _ => return Err(format!("{flag} is anneal or analytic, not {value:?}")),
                }
            }
            "--seed" => args.options.seed = number(&value)?,
            "--inner-num" => args.options.inner_num = factor(&value)?,
            "--tradeoff" => args.options.timing_tradeoff = factor(&value)?,
            "--detailed-temperature" => args.options.detailed_temperature = factor(&value)?,
            "--detailed-range" => args.options.detailed_range = number(&value)? as u32,
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    for (name, value) in [
        ("--arch", &args.arch),
        ("--rr-graph", &args.rr_graph),
        ("--net", &args.net),
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

fn slack_text(slack: Option<i64>) -> String {
    slack.map_or_else(|| "none".to_owned(), |slack| slack.to_string())
}

fn run(args: &Args) -> Result<bool, String> {
    let started = Instant::now();
    let arch = read_architecture(open(&args.arch)?).map_err(|e| format!("{}: {e}", args.arch))?;
    let device =
        read_rr_graph(open(&args.rr_graph)?).map_err(|e| format!("{}: {e}", args.rr_graph))?;
    let netlist = read_netlist(open(&args.net)?).map_err(|e| format!("{}: {e}", args.net))?;
    let fixed = match &args.fix {
        Some(path) => Some(read_placement(open(path)?).map_err(|e| format!("{path}: {e}"))?),
        None => None,
    };
    let mut problem_options = ProblemOptions::default();
    problem_options.fixed = fixed.as_ref();
    problem_options.ignored_nets = constant_nets(&netlist);
    let problem = build_place_problem(&device, &netlist, &arch, &problem_options)
        .map_err(|e| format!("building the problem: {e}"))?;
    let read_seconds = started.elapsed().as_secs_f64();

    let started = Instant::now();
    let delays = PlaceDelayModel::from_rr_graph(&device);
    let delay_model_seconds = started.elapsed().as_secs_f64();
    drop(device);

    let mut timing = build_timing(&netlist, arch.delays(), &TimingOptions::default())
        .map_err(|e| format!("timing graph: {e}"))?;
    let constraints = timing
        .vpr_constraints(0)
        .map_err(|e| format!("constraints: {e}"))?;

    let started = Instant::now();
    let result = if args.timing {
        place_design(
            &problem,
            &delays,
            Some((&mut timing, &constraints)),
            &args.options,
        )
    } else {
        place_design(&problem, &delays, None, &args.options)
    }
    .map_err(|e| format!("placing: {e}"))?;
    let place_seconds = started.elapsed().as_secs_f64();

    let violations = check(problem.problem(), &result.placement);
    let legal = violations.is_empty();
    let slack = estimate_slack(
        &problem,
        &delays,
        &mut timing,
        &constraints,
        &result.placement,
    );

    let mut evaluated = String::new();
    if let Some(path) = &args.evaluate {
        let other = read_placement(open(path)?).map_err(|e| format!("{path}: {e}"))?;
        let other = problem
            .from_placement(&netlist, &other)
            .map_err(|e| format!("{path}: {e}"))?;
        let other_slack = estimate_slack(&problem, &delays, &mut timing, &constraints, &other);
        evaluated = format!(
            " other_bb_estimate={:.0} other_worst_slack_ps={} other_legal={}",
            problem.bb_estimate(&other),
            slack_text(other_slack),
            check(problem.problem(), &other).is_empty()
        );
    }

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

    let (chains, chained) = problem.chains();
    println!(
        "legal={legal} violations={} blocks={} nets={} chains={chains} chained_blocks={chained} \
         bb_estimate={:.0} start_bb_estimate={:.0} worst_slack_ps={} temperatures={} moves={} \
         read_s={read_seconds:.3} delay_model_s={delay_model_seconds:.3} \
         place_s={place_seconds:.3} written={written}{evaluated}",
        violations.len(),
        netlist.blocks().len(),
        problem.problem().net_count(),
        result.bb_estimate,
        result.start_bb_estimate,
        slack_text(slack),
        result.temperatures,
        result.moves,
    );
    Ok(legal)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("vtr_place: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(message) => {
            eprintln!("vtr_place: {message}");
            ExitCode::from(2)
        }
    }
}
