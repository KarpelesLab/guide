//! Routes one of reticle's designs with reticle's router and with guide's, on one placement
//! by reticle, and checks both results with reticle's own checks.
//!
//! ```text
//! reticle-route --device <device> [--top <module>] --constraints <file> [options] <sources...>
//!
//!   --routers <list>        the routers, comma-separated (default reticle,guide):
//!                           `reticle`, or `guide[@<threads>][:<key>=<value>...]` with
//!                           keys `aim` (EngineOptions::target_terminals, 1 here),
//!                           `tile` (the lower bound's cost of a tile, reticle's
//!                           astar_weight here), `weight`, `growth`, `first`, `initial`,
//!                           `history` (reticle's own for the family here), `batch`
//!                           (32 with threads), `full` (no incremental rerouting),
//!                           `timed` (route by reticle's timing model, with
//!                           --timing-driven), `delay` (what delay weighs, over reticle's
//!                           scale) and `after` (EngineOptions::timing_iterations)
//!   --name <name>           the design's name in the results
//!   --place-seed <n>        the placer's seed
//!   --timing-driven         on a 7-series part, place and route reticle's way with its
//!                           timing model, as `reticle fpga --timing-driven` does (guide
//!                           still routes for wire alone)
//! ```
//!
//! Prints one line of JSON per router to standard output; notes go to standard error.

mod ecp5;
mod front;
mod generic;
mod router;
mod space;
mod timed;
mod xc7;

use std::sync::OnceLock;

use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::place::{Netlist, Placement};
use reticle::fpga::route::{RouteOptions, Routing};

use crate::router::{GuideConfig, RunReport};

/// A router to run, and how.
#[derive(Clone, Debug)]
pub enum Router {
    Reticle,
    Guide(GuideConfig),
}

impl Router {
    pub fn name(&self) -> String {
        match self {
            Router::Reticle => "reticle".into(),
            Router::Guide(config) => config.label.clone(),
        }
    }
}

/// Runs a router.
pub fn run_router(
    router: &Router,
    netlist: &Netlist,
    graph: &RoutingGraph,
    placement: &Placement,
    options: &RouteOptions,
) -> Result<(Routing, RunReport), String> {
    match router {
        Router::Reticle => router::route_reticle(netlist, graph, placement, options),
        Router::Guide(config) => router::route_guide(netlist, graph, placement, options, config),
    }
}

/// One router's result, checked.
#[derive(Debug, Default)]
pub struct Checked {
    pub router: String,
    pub report: RunReport,
    /// The latest path of each clock domain, in picoseconds, by reticle's model; or the
    /// longest path in the unit-delay model, named `units`.
    pub critical: Vec<(String, u64)>,
    pub notes: Vec<String>,
    pub problems: Vec<String>,
    pub failed: Option<String>,
    pub place_seconds: f64,
    pub graph_nodes: usize,
    pub graph_pips: usize,
    pub signals: usize,
    /// The finished routing's pips, those of them with configuration bits (the rest are
    /// the joins between a wire's names and fixed connections), and the graph nodes it holds.
    pub pips: usize,
    pub programmable: usize,
    pub wire_nodes: usize,
}

/// Counts what a routing uses, the same way for every router.
pub fn measure(checked: &mut Checked, graph: &RoutingGraph, routing: &Routing) {
    let mut nodes = std::collections::HashSet::new();
    let (mut pips, mut programmable) = (0, 0);
    for route in routing.routes() {
        nodes.insert(route.source);
        for pip in &route.pips {
            pips += 1;
            if !graph.pip_bits(*pip).is_empty() {
                programmable += 1;
            }
            nodes.insert(graph.pip(*pip).to);
        }
    }
    checked.pips = pips;
    checked.programmable = programmable;
    checked.wire_nodes = nodes.len();
}

impl Checked {
    pub fn new(router: &Router, graph: &RoutingGraph, netlist: &Netlist) -> Checked {
        Checked {
            router: router.name(),
            graph_nodes: graph.nodes.len(),
            graph_pips: graph.pips.len(),
            signals: netlist.routable().len(),
            ..Checked::default()
        }
    }

    fn legal(&self) -> bool {
        self.failed.is_none() && self.problems.is_empty()
    }

    fn json(&self, design: &str, device: &str) -> String {
        let s = |v: &str| format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""));
        let r = &self.report;
        let mut fields = vec![
            format!("\"design\":{}", s(design)),
            format!("\"device\":{}", s(device)),
            format!(
                "\"router\":{}",
                s(if self.router.is_empty() {
                    &r.router
                } else {
                    &self.router
                })
            ),
            format!("\"legal\":{}", self.legal()),
            format!("\"graph_nodes\":{}", self.graph_nodes),
            format!("\"graph_pips\":{}", self.graph_pips),
            format!("\"signals\":{}", self.signals),
            format!("\"place_seconds\":{:.3}", self.place_seconds),
        ];
        if self.failed.is_none() {
            fields.extend([
                format!("\"seconds\":{:.3}", r.seconds),
                format!("\"setup_seconds\":{:.3}", r.setup_seconds),
                format!("\"threads\":{}", r.threads),
                format!("\"iterations\":{}", r.iterations),
                format!("\"pips\":{}", self.pips),
                format!("\"programmable\":{}", self.programmable),
                format!("\"nodes\":{}", self.wire_nodes),
                format!("\"expansions\":{}", r.expansions),
                format!("\"rss_before_kib\":{}", r.rss_before_kib),
                format!("\"rss_peak_kib\":{}", r.rss_peak_kib),
                format!("\"replay_seconds\":{:.3}", r.replay_seconds),
            ]);
            if let Some((same, of)) = r.exact {
                fields.push(format!("\"exact\":[{same},{of}]"));
            }
            let critical: Vec<String> = self
                .critical
                .iter()
                .map(|(name, ps)| format!("{}:{ps}", s(name)))
                .collect();
            fields.push(format!("\"critical\":{{{}}}", critical.join(",")));
        }
        if let Some(error) = &self.failed {
            fields.push(format!("\"failed\":{}", s(error)));
        }
        if !self.problems.is_empty() {
            fields.push(format!("\"problems\":{}", s(&self.problems.join("; "))));
        }
        if !self.notes.is_empty() {
            fields.push(format!("\"notes\":{}", s(&self.notes.join("; "))));
        }
        format!("{{{}}}", fields.join(","))
    }
}

/// `reticle`, or `guide[@threads][:key=value...]`, the keys being `tile`, `weight`, `growth`,
/// `first`, `initial`, `history`, `batch` and `full` (no value).
fn parse_router(text: &str, base: &GuideConfig) -> Router {
    let mut parts = text.split(':');
    let head = parts.next().unwrap_or("");
    let bad = || -> ! {
        eprintln!("unknown router {text}");
        std::process::exit(2)
    };
    let (name, threads) = match head.split_once('@') {
        Some((name, threads)) => (name, threads.parse::<usize>().unwrap_or_else(|_| bad())),
        None => (head, 1),
    };
    if name == "reticle" {
        return Router::Reticle;
    }
    if name != "guide" {
        bad();
    }
    let mut config = base.clone();
    config.threads = threads;
    if threads > 1 && config.batch_size <= 1 {
        config.batch_size = 32;
    }
    for part in parts {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let number = || value.parse::<f64>().unwrap_or_else(|_| bad());
        match key {
            "tile" => config.per_tile = Some(number()),
            "weight" => config.bound_weight = number(),
            "growth" => config.present_growth = Some(number()),
            "first" => config.first_present = Some(number()),
            "initial" => config.initial_present = Some(number()),
            "history" => config.history_factor = Some(number()),
            "batch" => config.batch_size = number() as u32,
            "full" => config.incremental = false,
            "timed" => config.timed = true,
            "delay" => config.delay_weight = number(),
            "after" => config.timing_iterations = number() as u32,
            "aim" => config.target_terminals = number() as u32,
            _ => bad(),
        }
    }
    config.label = text.to_owned();
    Router::Guide(config)
}

static PLACE_SEED: OnceLock<Option<u64>> = OnceLock::new();
static TIMING_DRIVEN: OnceLock<bool> = OnceLock::new();

/// Whether `--timing-driven` was given.
pub fn timing_driven() -> bool {
    TIMING_DRIVEN.get().copied().unwrap_or(false)
}

/// The placer's seed, if one was given.
pub fn place_seed() -> Option<u64> {
    PLACE_SEED.get().copied().flatten()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut device = None;
    let mut top = None;
    let mut constraints = Vec::new();
    let mut sources = Vec::new();
    let mut routers_text = "reticle,guide".to_owned();
    let mut name = None;
    let config = GuideConfig::default();
    let mut seed = None;
    let mut timing = false;
    let mut at = 0;
    let value = |at: &mut usize| -> String {
        *at += 1;
        args.get(*at).cloned().unwrap_or_else(|| {
            eprintln!("{} needs a value", args[*at - 1]);
            std::process::exit(2)
        })
    };
    let number = |text: String| -> f64 {
        text.parse().unwrap_or_else(|_| {
            eprintln!("`{text}` is not a number");
            std::process::exit(2)
        })
    };
    while at < args.len() {
        match args[at].as_str() {
            "--device" => device = Some(value(&mut at)),
            "--top" => top = Some(value(&mut at)),
            "--constraints" => constraints.push(value(&mut at)),
            "--routers" => routers_text = value(&mut at),
            "--name" => name = Some(value(&mut at)),
            "--timing-driven" => timing = true,
            "--place-seed" => seed = Some(number(value(&mut at)) as u64),
            other if other.starts_with("--") => {
                eprintln!("unknown option {other}");
                std::process::exit(2)
            }
            source => sources.push(source.to_owned()),
        }
        at += 1;
    }
    let _ = PLACE_SEED.set(seed);
    let _ = TIMING_DRIVEN.set(timing);
    let Some(device) = device else {
        eprintln!("no --device");
        std::process::exit(2)
    };
    if sources.is_empty() {
        eprintln!("no sources");
        std::process::exit(2)
    }
    let routers: Vec<Router> = routers_text
        .split(',')
        .map(|r| parse_router(r, &config))
        .collect();
    let design_name = name.unwrap_or_else(|| {
        std::path::Path::new(&sources[0])
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
    });

    let started = std::time::Instant::now();
    let synthesised = match front::synthesise(&sources, top.as_deref(), &device, &constraints) {
        Ok(s) => s,
        Err(error) => {
            eprintln!("error: synthesis: {error}");
            std::process::exit(1)
        }
    };
    eprintln!(
        "note: synthesised {design_name} in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let results = match synthesised.device.family.as_str() {
        "xc7" => xc7::run(&synthesised, &routers),
        "ecp5" => ecp5::run(&synthesised, &routers),
        "ice40" | "gowin" => generic::run(&synthesised, &routers),
        other => Err(format!("family `{other}` is not handled")),
    };
    match results {
        Ok(results) => {
            for result in results {
                println!("{}", result.json(&design_name, &device));
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1)
        }
    }
}
