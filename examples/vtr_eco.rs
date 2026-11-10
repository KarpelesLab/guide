//! Measures engineering changes on a design that VPR packed and placed: small synthetic changes,
//! each handled from scratch and incrementally, with how much of the layout changed and what it
//! cost.
//!
//! This is the FPGA half of the measurements of engineering changes in `docs/benchmarks.md`.
//!
//! ```text
//! cargo run --release --features fpga --example vtr_eco -- \
//!     --arch k6_N10_40nm.xml --rr-graph rr.xml --net tseng.net --place tseng.place
//! ```
//!
//! The changes, each with a seed:
//!
//! - `route_nets`: of the nets that need routing, 1% and 5% change, a third of them added, a
//!   third removed, and a third given a sink more. The routes before are those of the design
//!   before the change.
//! - `route_moves`: 1% and 5% of the blocks of the most common type that are in no carry chain
//!   swap places in pairs, each with one at most 4 tiles away, so that their nets' terminals
//!   move. The routes before are those of the design as placed.
//! - `place_blocks`: 1% and 5% of those blocks are added: the placement before is of the
//!   netlist without them, by analytic placement on wirelength, and so is the placement from
//!   scratch of the whole netlist. Each placement is then routed from scratch.
//!
//! Routing is timing-driven, with `RrGraph::engine_options`, and incremental routing adds
//! `eco::stable_options` to those. Each line printed is one result, as `key=value` pairs.
//!
//! Options: `--arch`, `--rr-graph`, `--net` and `--place` as for `vtr_route`, `--seed N` (1),
//! `--threads N` and `--batch N` (1), `--no-timing`, and `--only KIND` for one kind of change.

use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;
use std::time::Instant;

use guide::eco::{route_change, stable_options};
use guide::engine::{Engine, EngineOptions, Outcome};
use guide::fpga::{
    Design, DesignTiming, LookaheadOptions, NetOptions, NodeKind, PackedNetlist, PlaceProblem,
    PlacedBlock, Placement, ProblemOptions, RrGraph, TimingOptions, build_place_problem,
    build_timing, constant_nets, read_architecture, read_netlist, read_placement, read_rr_graph,
    resolve_nets, route_seeds,
};
use guide::id::{NetId, NodeId};
use guide::net::{Net, RouteSet};
use guide::place::{
    AnalyticOptions, IncrementalOptions, ItemId, Wirelength, check, place_analytic,
    place_incremental, placement_change, restrict,
};
use guide::rng::Rng;
use guide::timing::Constraints;

struct Args {
    arch: String,
    rr_graph: String,
    net: String,
    place: String,
    seed: u64,
    threads: usize,
    batch: u32,
    timing: bool,
    only: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        arch: String::new(),
        rr_graph: String::new(),
        net: String::new(),
        place: String::new(),
        seed: 1,
        threads: 1,
        batch: 1,
        timing: true,
        only: None,
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
        let number = |what: &str| value.parse::<u64>().map_err(|_| format!("bad {what}"));
        match flag.as_str() {
            "--arch" => args.arch = value,
            "--rr-graph" => args.rr_graph = value,
            "--net" => args.net = value,
            "--place" => args.place = value,
            "--seed" => args.seed = number("seed")?,
            "--threads" => args.threads = number("thread count")? as usize,
            "--batch" => args.batch = number("batch size")? as u32,
            "--only" => args.only = Some(value),
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if args.arch.is_empty() || args.rr_graph.is_empty() || args.net.is_empty() {
        return Err("--arch, --rr-graph, --net and --place are required".to_owned());
    }
    if args.place.is_empty() {
        return Err("--place is required".to_owned());
    }
    Ok(args)
}

fn open(path: &str) -> Result<BufReader<File>, String> {
    File::open(path)
        .map(|file| BufReader::with_capacity(1 << 20, file))
        .map_err(|error| format!("{path}: {error}"))
}

/// The wirelength as VPR counts it: the tiles spanned by every wire that a route uses.
fn wirelength(device: &RrGraph, routes: &RouteSet<NodeId>) -> u64 {
    let mut total = 0;
    for (_, route) in routes.routes() {
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

/// What is the same for every run.
struct Context<'a> {
    device: &'a RrGraph,
    netlist: &'a PackedNetlist,
    timing: &'a DesignTiming,
    constraints: &'a Constraints,
    net_options: &'a NetOptions,
    args: &'a Args,
}

/// One routing run, and what came of it.
struct Routed {
    outcome: Outcome<NodeId, NodeId>,
    seconds: f64,
}

impl Context<'_> {
    fn design(&self, placement: &Placement) -> Result<Design, String> {
        resolve_nets(self.device, self.netlist, placement, self.net_options)
            .map_err(|e| format!("resolving nets: {e}"))
    }

    /// Routes `nets` of `design`, from `seeds` if given.
    fn route(
        &self,
        design: &Design,
        nets: &[Net<NodeId>],
        seeds: Option<Vec<(NetId, guide::net::Route<NodeId>)>>,
    ) -> Result<Routed, String> {
        let mut options: EngineOptions<'static> = self.device.engine_options();
        options.batch_size = self.args.batch;
        if seeds.is_some() {
            options = stable_options(options);
        }
        let started = Instant::now();
        let timed = self.device.timed_graph();
        let outcome = if self.args.timing {
            let mut timing = self.timing.clone();
            timing.estimate_delays(&timed, design);
            let mut driven = timing.timing_driven(design, self.constraints.clone());
            // Only the connections of the nets routed here, and of sinks they have.
            let sinks: std::collections::HashMap<NetId, u32> = nets
                .iter()
                .map(|net| (net.id, net.terminals.len() as u32))
                .collect();
            driven
                .connections
                .retain(|c| sinks.get(&c.net).is_some_and(|&n| c.terminal < n));
            let mut engine =
                Engine::with_timing(&timed, nets, options, driven).map_err(|e| e.to_string())?;
            if let Some(seeds) = seeds {
                engine.seed_routes(seeds).map_err(|e| e.to_string())?;
            }
            while engine.step_parallel(self.args.threads).is_some() {}
            engine.outcome()
        } else {
            let mut engine =
                Engine::new(self.device.graph(), nets, options).map_err(|e| e.to_string())?;
            if let Some(seeds) = seeds {
                engine.seed_routes(seeds).map_err(|e| e.to_string())?;
            }
            while engine.step_parallel(self.args.threads).is_some() {}
            engine.outcome()
        };
        Ok(Routed {
            outcome,
            seconds: started.elapsed().as_secs_f64(),
        })
    }

    /// Prints one routing result against the routes before.
    fn report_route(&self, line: &str, before: &RouteSet<NodeId>, routed: &Routed) {
        let outcome = &routed.outcome;
        let change = route_change(before, &outcome.routes);
        let critical = outcome
            .timing
            .as_ref()
            .and_then(|t| t.metrics.critical_path_delay)
            .map_or_else(|| "none".to_owned(), |ps| ps.to_string());
        println!(
            "{line} legal={} unchanged={:.4} nodes_changed={:.4} compared={} wirelength={} \
             critical_path_ps={critical} expansions={} iterations={} route_s={:.3}",
            outcome.is_success(),
            change.unchanged_fraction(),
            change.nodes_changed_fraction(),
            change.nets_compared,
            wirelength(self.device, &outcome.routes),
            outcome.routes.stats.nodes_expanded,
            outcome.routes.stats.iterations,
            routed.seconds,
        );
    }
}

/// Of every hundred nets, `percent` change: a third added, a third removed, and a third given a
/// sink more, as `(before, after)`.
fn changed_nets(
    nets: &[Net<NodeId>],
    percent: u32,
    seed: u64,
) -> (Vec<Net<NodeId>>, Vec<Net<NodeId>>) {
    let routed: Vec<usize> = (0..nets.len())
        .filter(|&i| nets[i].terminals.len() >= 2)
        .collect();
    let mut order = routed.clone();
    Rng::new(seed).shuffle(&mut order);
    let count = (routed.len() * percent as usize).div_ceil(100).max(3);
    let mut change = vec![0u8; nets.len()];
    for (k, &index) in order.iter().take(count).enumerate() {
        change[index] = 1 + (k % 3) as u8;
    }
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (net, &kind) in nets.iter().zip(&change) {
        match kind {
            1 => after.push(net.clone()),
            2 => before.push(net.clone()),
            3 if net.terminals.len() > 2 => {
                let mut fewer = net.clone();
                fewer.terminals.pop();
                before.push(fewer);
                after.push(net.clone());
            }
            _ => {
                before.push(net.clone());
                after.push(net.clone());
            }
        }
    }
    (before, after)
}

/// The blocks of the most common type, by number, that are in no carry chain, which have to stay
/// where the chain's direct connections can reach them.
fn common_blocks(netlist: &PackedNetlist, problem: &PlaceProblem) -> Vec<usize> {
    let in_chain: std::collections::HashSet<ItemId> = problem
        .problem()
        .macros()
        .iter()
        .flat_map(|members| members.iter().map(|&(item, _)| item))
        .collect();
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for block in netlist.blocks() {
        *counts.entry(block.pb_type.as_str()).or_default() += 1;
    }
    let common = counts
        .iter()
        .max_by_key(|&(name, &count)| (count, std::cmp::Reverse(*name)))
        .map(|(&name, _)| name)
        .unwrap_or("");
    (0..netlist.blocks().len())
        .filter(|&i| netlist.blocks()[i].pb_type == common)
        .filter(|&i| !in_chain.contains(&problem.item(i as u32)))
        .collect()
}

/// How far apart, in tiles, two blocks that swap places may be.
const SWAP_REACH: u32 = 4;

/// `placement` with `percent` of the blocks of the most common type swapped in pairs, each with
/// a block at most [`SWAP_REACH`] tiles away, as a small change to a placement would.
fn swapped(
    netlist: &PackedNetlist,
    problem: &PlaceProblem,
    placement: &Placement,
    percent: u32,
    seed: u64,
) -> Placement {
    let mut rng = Rng::new(seed);
    let mut blocks = common_blocks(netlist, problem);
    rng.shuffle(&mut blocks);
    let count = (blocks.len() * percent as usize).div_ceil(100).max(2);
    let mut place_of: std::collections::HashMap<&str, PlacedBlock> = placement
        .blocks()
        .iter()
        .map(|block| (block.name.as_str(), block.clone()))
        .collect();
    let at = |block: usize| {
        placement
            .block(&netlist.blocks()[block].name)
            .map(|placed| (placed.x, placed.y))
    };
    let mut used = vec![false; netlist.blocks().len()];
    let mut pairs = Vec::new();
    for &a in &blocks {
        if 2 * pairs.len() >= count {
            break;
        }
        let Some((ax, ay)) = at(a).filter(|_| !used[a]) else {
            continue;
        };
        let near: Vec<usize> = blocks
            .iter()
            .copied()
            .filter(|&b| b != a && !used[b])
            .filter(|&b| {
                at(b).is_some_and(|(bx, by)| ax.abs_diff(bx) + ay.abs_diff(by) <= SWAP_REACH)
            })
            .collect();
        if near.is_empty() {
            continue;
        }
        let b = near[rng.below(near.len() as u64) as usize];
        used[a] = true;
        used[b] = true;
        pairs.push([a, b]);
    }
    for pair in pairs {
        let (a, b) = (
            &netlist.blocks()[pair[0]].name,
            &netlist.blocks()[pair[1]].name,
        );
        let (Some(pa), Some(pb)) = (
            place_of.get(a.as_str()).cloned(),
            place_of.get(b.as_str()).cloned(),
        ) else {
            continue;
        };
        place_of.insert(
            a,
            PlacedBlock {
                name: a.clone(),
                ..pb
            },
        );
        place_of.insert(
            b,
            PlacedBlock {
                name: b.clone(),
                ..pa
            },
        );
    }
    let mut out = Placement::new(None, None, placement.array_size());
    for block in placement.blocks() {
        out.add_block(place_of[block.name.as_str()].clone());
    }
    out
}

fn run(args: &Args) -> Result<(), String> {
    let arch = read_architecture(open(&args.arch)?).map_err(|e| format!("{}: {e}", args.arch))?;
    let mut device =
        read_rr_graph(open(&args.rr_graph)?).map_err(|e| format!("{}: {e}", args.rr_graph))?;
    let netlist = read_netlist(open(&args.net)?).map_err(|e| format!("{}: {e}", args.net))?;
    let placement =
        read_placement(open(&args.place)?).map_err(|e| format!("{}: {e}", args.place))?;
    let mut net_options = NetOptions::default();
    net_options.constant_nets = constant_nets(&netlist);
    device.use_map_lookahead(&LookaheadOptions::default());
    let timing = build_timing(&netlist, arch.delays(), &TimingOptions::default())
        .map_err(|e| format!("timing graph: {e}"))?;
    let constraints = timing
        .vpr_constraints(0)
        .map_err(|e| format!("constraints: {e}"))?;
    let context = Context {
        device: &device,
        netlist: &netlist,
        timing: &timing,
        constraints: &constraints,
        net_options: &net_options,
        args,
    };
    let wants = |kind: &str| args.only.as_deref().is_none_or(|only| only == kind);
    let design = context.design(&placement)?;
    let seed = args.seed;
    let mut problem_options = ProblemOptions::default();
    problem_options.ignored_nets = constant_nets(&netlist);
    let problem: PlaceProblem = build_place_problem(&device, &netlist, &arch, &problem_options)
        .map_err(|e| format!("building the problem: {e}"))?;

    if wants("route_nets") {
        for percent in [1, 5] {
            let (before, after) = changed_nets(design.nets(), percent, seed);
            let base = context.route(&design, &before, None)?;
            let line = format!("change=route_nets percent={percent} seed={seed}");
            let scratch = context.route(&design, &after, None)?;
            context.report_route(
                &format!("{line} method=scratch"),
                &base.outcome.routes,
                &scratch,
            );
            let seeds = route_seeds(&design, &base.outcome.routes, &design);
            let eco = context.route(&design, &after, Some(seeds))?;
            context.report_route(
                &format!("{line} method=incremental"),
                &base.outcome.routes,
                &eco,
            );
        }
    }

    if wants("route_moves") {
        let base = context.route(&design, design.nets(), None)?;
        for percent in [1, 5] {
            let moved = swapped(&netlist, &problem, &placement, percent, seed);
            let changed = context.design(&moved)?;
            let line = format!("change=route_moves percent={percent} seed={seed}");
            let scratch = context.route(&changed, changed.nets(), None)?;
            context.report_route(
                &format!("{line} method=scratch"),
                &base.outcome.routes,
                &scratch,
            );
            let seeds = route_seeds(&design, &base.outcome.routes, &changed);
            let eco = context.route(&changed, changed.nets(), Some(seeds))?;
            context.report_route(
                &format!("{line} method=incremental"),
                &base.outcome.routes,
                &eco,
            );
        }
    }

    if wants("place_blocks") {
        let whole = problem.problem();
        for percent in [1, 5] {
            let mut blocks = common_blocks(&netlist, &problem);
            Rng::new(seed).shuffle(&mut blocks);
            let count = (blocks.len() * percent as usize).div_ceil(100).max(1);
            let mut keep = vec![true; whole.items().len()];
            for &b in &blocks[..count] {
                keep[problem.item(b as u32).index()] = false;
            }
            let restricted = restrict(whole, &keep, &vec![true; whole.net_count()]);
            let mut options = AnalyticOptions::default();
            options.seed = seed;
            let (base, _) = place_analytic(&restricted.problem, Wirelength, options)
                .map_err(|e| format!("placing: {e}"))?;
            let priors = restricted.priors(&base.placement);
            let line = format!("change=place_blocks percent={percent} seed={seed}");
            for method in ["scratch", "incremental"] {
                let started = Instant::now();
                let placed = if method == "scratch" {
                    place_analytic(whole, Wirelength, options)
                        .map_err(|e| format!("placing: {e}"))?
                        .0
                        .placement
                } else {
                    place_incremental(whole, Wirelength, &priors, IncrementalOptions::default())
                        .map_err(|e| format!("placing: {e}"))?
                        .outcome
                        .placement
                };
                let place_seconds = started.elapsed().as_secs_f64();
                let legal = check(whole, &placed).is_empty();
                let change = placement_change(whole, &priors, &placed);
                let vpr = problem.to_placement(&netlist, &placed, "", "");
                let placed_design = context.design(&vpr)?;
                let routed = context.route(&placed_design, placed_design.nets(), None)?;
                let critical = routed
                    .outcome
                    .timing
                    .as_ref()
                    .and_then(|t| t.metrics.critical_path_delay)
                    .map_or_else(|| "none".to_owned(), |ps| ps.to_string());
                println!(
                    "{line} method={method} legal={legal} bb_estimate={:.1} moved={:.4} \
                     mean_displacement={:.3} max_displacement={} added={} place_s={place_seconds:.3} \
                     route_legal={} wirelength={} critical_path_ps={critical} route_s={:.3}",
                    problem.bb_estimate(&placed),
                    change.moved_fraction(),
                    change.mean_displacement(),
                    change.max_displacement,
                    change.added,
                    routed.outcome.is_success(),
                    wirelength(&device, &routed.outcome.routes),
                    routed.seconds,
                );
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("vtr_eco: {message}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("vtr_eco: {message}");
            ExitCode::from(2)
        }
    }
}
