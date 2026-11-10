//! Routes an ISPD 2007 or 2008 global routing contest design, writes the routes in the
//! contests' format, and scores them as the 2008 contest's `eval2008.pl` does.
//!
//! This is the harness for `scripts/ispd/` and the ASIC section of `docs/benchmarks.md`.
//!
//! ```text
//! cargo run --release --features asic --example ispd_route -- adaptec1.capo70.3d.35.50.90.gr \
//!     --out adaptec1.route --threads 16 --batch 64
//! cargo run --release --features asic --example ispd_route -- adaptec1.capo70.3d.35.50.90.gr \
//!     --score adaptec1.route
//! ```
//!
//! | Option | Meaning | Default |
//! | --- | --- | --- |
//! | `--out FILE` | Where to write the routes | Not written |
//! | `--score FILE` | Score this route file instead of routing | Route |
//! | `--via-cost N` | Wirelength per layer a via spans: 1 in 2008, 3 in 2007 | 1 |
//! | `--threads N` | `AsicOptions::threads` | 1 |
//! | `--batch N` | `AsicOptions::batch_size` | 32 |
//! | `--max-iterations N` | `AsicOptions::max_iterations` | 100 |
//! | `--layer-passes N` | `AsicOptions::layer_passes` | 4 |
//! | `--box-margin N` | `AsicOptions::box_margin`, -1 for none | 10 |
//! | `--box-growth N` | `AsicOptions::box_growth` | 5 |
//! | `--first-present F` | `AsicOptions::first_present` | 0.5 |
//! | `--initial-present F` | `AsicOptions::initial_present` | 0.5 |
//! | `--present-growth F` | `AsicOptions::present_growth` | 1.8 |
//! | `--history F` | `AsicOptions::history_factor` | 1 |
//! | `--bend F` | `AsicOptions::bend_cost` | 1 |
//! | `--no-pattern` | `AsicOptions::pattern` off | On |
//! | `--crowding F` | `AsicOptions::crowding` | 1 |
//!
//! It prints one line of `key=value` pairs, which the scripts parse.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::time::Instant;

use guide::asic::{AsicOptions, NetRoute, evaluate, read_design, read_routes, route_design};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut design_path = None;
    let mut out = None;
    let mut score = None;
    let mut via_cost = 1u64;
    let mut options = AsicOptions::default();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(value()?),
            "--score" => score = Some(value()?),
            "--via-cost" => via_cost = value()?.parse()?,
            "--threads" => options.threads = value()?.parse()?,
            "--batch" => options.batch_size = value()?.parse()?,
            "--max-iterations" => options.max_iterations = value()?.parse()?,
            "--layer-passes" => options.layer_passes = value()?.parse()?,
            "--box-margin" => {
                let v: i64 = value()?.parse()?;
                options.box_margin = u32::try_from(v).ok();
            }
            "--box-growth" => options.box_growth = value()?.parse()?,
            "--first-present" => options.first_present = value()?.parse()?,
            "--initial-present" => options.initial_present = value()?.parse()?,
            "--present-growth" => options.present_growth = value()?.parse()?,
            "--history" => options.history_factor = value()?.parse()?,
            "--bend" => options.bend_cost = value()?.parse()?,
            "--no-pattern" => options.pattern = false,
            "--crowding" => options.crowding = value()?.parse()?,
            _ if !arg.starts_with("--") && design_path.is_none() => design_path = Some(arg),
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    let design_path = design_path.ok_or("usage: ispd_route DESIGN.gr [options]")?;
    let start = Instant::now();
    let design = read_design(BufReader::new(File::open(&design_path)?))?;
    let read_time = start.elapsed().as_secs_f64();

    let (routes, route_time, extra): (Vec<NetRoute>, f64, String) = if let Some(path) = score {
        let routes = read_routes(BufReader::new(File::open(path)?))?;
        (routes, 0.0, String::new())
    } else {
        let start = Instant::now();
        let routing = route_design(&design, &options);
        let route_time = start.elapsed().as_secs_f64();
        let mut text = Vec::new();
        for (net, segments) in design.nets.iter().zip(&routing.routes) {
            if !segments.is_empty() {
                guide::asic::write_net_route(&mut text, &design, net, segments)?;
            }
        }
        if let Some(path) = out {
            BufWriter::new(File::create(path)?).write_all(&text)?;
        }
        if std::env::var_os("ISPD_VERBOSE").is_some() {
            for r in &routing.iterations {
                eprintln!(
                    "iteration={} rerouted={} searches={} expanded={} overused={} overuse={} cost={:.0}",
                    r.iteration,
                    r.nets_rerouted,
                    r.searches,
                    r.nodes_expanded,
                    r.overused_resources,
                    r.total_overuse,
                    r.total_cost.get()
                );
            }
        }
        let last = routing.iterations.last();
        let extra = format!(
            " routed_nets={} iterations={} planar_overflow={} expanded={}",
            routing.routed_nets,
            routing.iterations.len(),
            routing.planar_overflow,
            routing
                .iterations
                .iter()
                .map(|r| r.nodes_expanded)
                .sum::<u64>(),
        ) + &last.map_or(String::new(), |r| {
            format!(" last_overused={}", r.overused_resources)
        });
        (read_routes(text.as_slice())?, route_time, extra)
    };
    let s = evaluate(&design, &routes, via_cost);
    println!(
        "design={} nets={} read_s={read_time:.2} route_s={route_time:.2} tof={} mof={} \
         overflowed_edges={} wl={} wire={} vias={} unrouted={} unattached={} faults={} cycles={}{extra}",
        design_path.rsplit('/').next().unwrap_or(&design_path),
        design.nets.len(),
        s.total_overflow,
        s.max_overflow,
        s.overflowed_edges,
        s.wirelength,
        s.wire_length,
        s.vias,
        s.unrouted.len(),
        s.unattached_pins,
        s.faults.len(),
        s.nets_with_cycles,
    );
    Ok(if s.is_complete() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
