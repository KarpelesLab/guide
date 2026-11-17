//! Places reticle's designs with guide, routes them with reticle's router, and compares
//! the result with reticle's own placement routed the same way.
//!
//! ```text
//! reticle-place run --device D [--constraints F]... [--top T] [--param K=V]...
//!     [--placer reticle|guide] [--seed N] [--inner-num X] SOURCES...
//! reticle-place inspect --device D ... SOURCES...
//! ```
//!
//! `run` prints one line of `key=value` pairs: what the placement took and measured, whether
//! reticle's router routed it and with how much, and whether every check after routing that
//! `reticle fpga --bitstream` makes passed.

mod ecp5;
mod flow;
mod front;
mod ice40;
mod inspect;
mod model;
mod placer;
mod xc7;

use std::collections::BTreeMap;
use std::time::Instant;

use reticle::fpga::place::{PlaceOptions, hpwl, shared_exits};

fn usage() -> ! {
    eprintln!(
        "usage: reticle-place run|inspect --device D [--constraints F]... [--top T] \
         [--param K=V]... [--placer reticle|guide] [--seed N] [--inner-num X] SOURCES..."
    );
    std::process::exit(2);
}

struct Args {
    command: String,
    device: String,
    constraints: Vec<String>,
    top: Option<String>,
    params: Vec<(String, String)>,
    sources: Vec<String>,
    options: BTreeMap<String, String>,
}

fn parse_args() -> Args {
    let mut raw = std::env::args().skip(1);
    let Some(command) = raw.next() else { usage() };
    let mut args = Args {
        command,
        device: String::new(),
        constraints: Vec::new(),
        top: None,
        params: Vec::new(),
        sources: Vec::new(),
        options: BTreeMap::new(),
    };
    while let Some(arg) = raw.next() {
        if let Some(name) = arg.strip_prefix("--") {
            let Some(value) = raw.next() else { usage() };
            match name {
                "device" => args.device = value,
                "constraints" => args.constraints.push(value),
                "top" => args.top = Some(value),
                "param" => {
                    let (k, v) = value.split_once('=').unwrap_or_else(|| usage());
                    args.params.push((k.to_owned(), v.to_owned()));
                }
                _ => {
                    args.options.insert(name.to_owned(), value);
                }
            }
        } else {
            args.sources.push(arg);
        }
    }
    args
}

impl Args {
    fn number<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        self.options
            .get(name)
            .map_or(default, |v| v.parse().unwrap_or_else(|_| usage()))
    }
}

/// What a run measured, printed as `key=value` pairs in order.
#[derive(Default)]
struct Line(Vec<(String, String)>);

impl Line {
    fn put(&mut self, key: &str, value: impl std::fmt::Display) {
        self.0.push((key.to_owned(), value.to_string()));
    }

    fn print(&self) {
        let text: Vec<String> = self
            .0
            .iter()
            .map(|(k, v)| format!("{k}={}", v.replace(' ', "_")))
            .collect();
        println!("{}", text.join(" "));
    }
}

fn fail(line: &mut Line, stage: &str, message: &str) -> ! {
    line.put("ok", "no");
    line.put("failed", stage);
    line.print();
    eprintln!("error: {stage}: {message}");
    std::process::exit(1);
}

/// A family's flow: its architecture, router options and checks.
enum Family {
    Ecp5(Box<ecp5::Ecp5>),
    Ice40(Box<ice40::Ice40>),
    Xc7(Box<xc7::Xc7>),
}

impl Family {
    fn arch(&self) -> &reticle::fpga::arch::Arch {
        match self {
            Family::Ecp5(f) => &f.fabric.arch,
            Family::Ice40(f) => &f.arch,
            Family::Xc7(f) => &f.fabric.arch,
        }
    }

    /// The design as the flow has it when it places: the 7 series changes it first.
    fn design<'d>(&'d self, design: &'d reticle::ir::Design) -> &'d reticle::ir::Design {
        match self {
            Family::Xc7(f) => &f.design,
            _ => design,
        }
    }
}

fn main() {
    let args = parse_args();
    let Some(device) = reticle::fpga::target(&args.device) else {
        eprintln!("unknown device `{}`", args.device);
        std::process::exit(2);
    };
    let mut line = Line::default();
    let placer_name = args
        .options
        .get("placer")
        .cloned()
        .unwrap_or_else(|| "guide".to_owned());
    let seed: u64 = args.number("seed", 1);
    line.put("placer", &placer_name);
    line.put("seed", seed);

    let started = Instant::now();
    let synthesised = match front::synthesise(
        &args.sources,
        &args.constraints,
        device,
        args.top.as_deref(),
        &args.params,
        args.options.get("ip").map(String::as_str),
    ) {
        Ok(s) => s,
        Err(e) => fail(&mut line, "synthesis", &e),
    };
    let (design, top, constraints) = (
        &synthesised.design,
        synthesised.top,
        &synthesised.constraints,
    );
    line.put("synth_s", format!("{:.2}", started.elapsed().as_secs_f64()));

    let started = Instant::now();
    let loaded = match device.family.as_str() {
        "ecp5" => ecp5::load(design, top, device, constraints)
            .map(|(f, l)| (Family::Ecp5(Box::new(f)), l)),
        "ice40" => ice40::load(design, top, device).map(|(f, l)| (Family::Ice40(Box::new(f)), l)),
        "xc7" => {
            xc7::load(design, top, device, constraints).map(|(f, l)| (Family::Xc7(Box::new(f)), l))
        }
        other => Err(format!("family `{other}` not supported yet")),
    };
    let (family, loaded) = match loaded {
        Ok(pair) => pair,
        Err(e) => fail(&mut line, "load", &e),
    };
    let (graph, netlist) = (&loaded.graph, &loaded.netlist);
    line.put("load_s", format!("{:.2}", started.elapsed().as_secs_f64()));
    line.put("cells", netlist.instances.len());
    line.put(
        "signals",
        netlist.signals.iter().filter(|s| s.is_routable()).count(),
    );

    if args.command == "inspect" {
        inspect::report(graph, netlist);
        return;
    }
    if args.command != "run" {
        usage();
    }

    let design = family.design(design);
    let place_options = match &family {
        Family::Xc7(f) => f.place_options(graph, netlist, top, seed),
        _ => PlaceOptions {
            seed,
            ..PlaceOptions::default()
        },
    };
    let started = Instant::now();
    let needed;
    let mut placement = match placer_name.as_str() {
        "reticle" => {
            let options = place_options.clone();
            match reticle::fpga::place::place(netlist, family.arch(), graph, constraints, &options)
            {
                Ok((placement, report)) => {
                    needed = report.route_throughs_needed;
                    placement
                }
                Err(e) => fail(&mut line, "place", &e.to_string()),
            }
        }
        "guide" => {
            // `--forbid SITE,...`: sites guide's placement leaves empty.
            let forbidden: Vec<String> = args
                .options
                .get("forbid")
                .map(|v| v.split(',').map(str::to_owned).collect())
                .unwrap_or_default();
            let built = match model::build(
                netlist,
                family.arch(),
                graph,
                constraints,
                &place_options,
                &forbidden,
            ) {
                Ok(m) => m,
                Err(e) => fail(&mut line, "model", &e),
            };
            line.put("model_s", format!("{:.2}", started.elapsed().as_secs_f64()));
            for (k, v) in &built.stats {
                line.put(&k.replace(' ', "_"), v);
            }
            let options = placer::GuideOptions {
                seed,
                inner_num: args.number("inner-num", 3.0),
                temperature: args.number("temperature", 0.5),
                density: args.number("density", 0.5),
                range: args.number("range", 8),
                anneal_only: args.number("anneal-only", 0) == 1,
            };
            match placer::place(&built, graph.sites.len(), options) {
                Ok((placement, report)) => {
                    needed = reticle::fpga::place::route_throughs_needed(
                        netlist,
                        graph,
                        &placement,
                        &place_options.route_throughs,
                    );
                    line.put("guide_legalized", report.legalized);
                    line.put("guide_moves", report.moves);
                    line.put("guide_cost", report.cost);
                    placement
                }
                Err(e) => fail(&mut line, "place", &e),
            }
        }
        _ => usage(),
    };
    if let Family::Xc7(f) = &family
        && let Err(e) = f.after_placement(graph, netlist, &mut placement)
    {
        fail(&mut line, "place", &e);
    }
    line.put("place_s", format!("{:.2}", started.elapsed().as_secs_f64()));
    line.put("throughs", needed.len());
    line.put("hpwl", hpwl(netlist, graph, &placement));
    let exits = shared_exits(netlist, graph, &placement);
    line.put("shared_exits", exits.len());

    let started = Instant::now();
    let route_options = match &family {
        Family::Ecp5(f) => f.route_options(graph),
        Family::Ice40(f) => f.route_options(),
        Family::Xc7(f) => match f.route_options(top, graph, netlist, &placement, &needed) {
            Ok(o) => o,
            Err(e) => fail(&mut line, "route", &e),
        },
    };
    let routed = reticle::fpga::route::route(netlist, graph, &placement, &route_options);
    line.put("route_s", format!("{:.2}", started.elapsed().as_secs_f64()));
    let (routing, report) = match routed {
        Ok(r) => r,
        Err(e) => fail(&mut line, "route", &e.to_string()),
    };
    line.put("iterations", report.iterations.len());
    line.put("pips", report.pips);
    line.put("nodes", report.nodes);
    line.put("visited", report.visited);
    let verified = match &family {
        Family::Xc7(f) => f.verify(top, graph, netlist, &placement, &routing),
        _ => {
            let problems = routing.verify(netlist, graph, &placement);
            if problems.is_empty() {
                Ok(())
            } else {
                Err(problems.join("; "))
            }
        }
    };
    if let Err(e) = verified {
        fail(&mut line, "verify", &e);
    }
    let checked = match &family {
        Family::Ecp5(f) => f.check(design, top, device, graph, netlist, &placement, &routing),
        Family::Ice40(f) => f.check(design, top, graph, netlist, &placement, &routing),
        Family::Xc7(f) => f.check(graph, netlist, &placement, &routing).map(|()| {
            let (timing, names) = f.timing(
                top,
                graph,
                netlist,
                constraints,
                &place_options.clock_regions,
            );
            let paths = f.routed_timing(&timing, graph, netlist, &placement, &routing, &needed);
            for (name, (ps, period)) in names.iter().zip(paths) {
                line.put(
                    &format!("path_{name}"),
                    format!("{:.3}/{period}", ps as f64 / 1000.0),
                );
            }
            flow::Checked { bits: 0, arcs: 0 }
        }),
    };
    match checked {
        Ok(checked) => {
            line.put("bits", checked.bits);
            line.put("arcs", checked.arcs);
        }
        Err(e) => fail(&mut line, "check", &e),
    }
    line.put("ok", "yes");
    line.print();
}
