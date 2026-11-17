//! The families reticle routes with its default options: iCE40, on its built-in synthetic
//! architecture (`fpga::flow::place_and_route`), and Gowin, by Project Apicula's database
//! (`write_gowin_bitstream` in `src/bin/reticle/main.rs`). Neither has a database to decode a
//! bitstream back through, so a result is checked by `Routing::verify`, `Routing::shorts` and
//! by generating the bitstream's tiles.

use std::collections::BTreeMap;

use reticle::fpga::arch::{Arch, RoutingGraph};
use reticle::fpga::route::{RouteOptions, Routing, Timing, latest_paths};
use reticle::fpga::{Netlist, bitstream, place};

use crate::front::{DiskFiles, Synthesised, database};
use crate::{Checked, Router, run_router};

/// The longest register-to-register path, one unit a graph node and nothing a cell, for a
/// family reticle has no delay model of.
pub fn unit_longest(
    netlist: &Netlist,
    graph: &RoutingGraph,
    placement: &place::Placement,
    routing: &Routing,
    units: &Timing,
) -> Vec<(String, u64)> {
    let paths = latest_paths(netlist, graph, placement, routing, units, 1);
    let longest = paths
        .first()
        .and_then(|p| p.last())
        .map_or(0, |step| step.arrival);
    vec![("units".to_owned(), longest)]
}

fn gowin_arch(s: &Synthesised) -> Result<Arch, String> {
    use reticle::fpga::apicula::{ApiculaDatabase, ApiculaOptions};
    use reticle::fpga::xray::GridRegion;
    const GOWIN_PARTS: &[(&str, &str, &str)] =
        &[("gw2a-18-pg256", "GW2A-18", "GW2A-LV18PG256C8/I7")];
    let Some(&(_, die, part)) = GOWIN_PARTS
        .iter()
        .find(|(name, _, _)| *name == s.device.name)
    else {
        return Err(format!("no Project Apicula part for `{}`", s.device.name));
    };
    let root = database("RETICLE_GOWINDB", "apicula")?;
    let db = ApiculaDatabase::open(&root, die, &DiskFiles::for_sources(&[]))
        .map_err(|e| e.to_string())?;
    let mut options = ApiculaOptions::new().with_part(part);
    let probe = db
        .load(&options.clone().with_region(GridRegion::new(0, 0, 0, 0)))
        .map_err(|e| e.to_string())?;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    let mut per_bank: BTreeMap<i64, String> = BTreeMap::new();
    for pin in &s.constraints.pins {
        let Some(site) = probe.arch.site_of_pin(&pin.pin) else {
            continue;
        };
        let Some(((x, y), bel)) = site.split_once('/').and_then(|(tile, bel)| {
            let (x, y) = tile.strip_prefix('X')?.split_once('Y')?;
            Some(((x.parse::<u32>().ok()?, y.parse::<u32>().ok()?), bel))
        }) else {
            continue;
        };
        if let (Some(standard), Some(io)) = (&pin.io.io_standard, probe.periphery.io((x, y), bel)) {
            per_bank.entry(io.bank).or_insert_with(|| standard.clone());
        }
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    let (x0, y0, x1, y1) = bounds.ok_or("no constrained pin names a ball of this part")?;
    let (cols, rows) = (probe.stats.cols, probe.stats.rows);
    let margin = 4;
    options.region = Some(GridRegion::new(
        x0.saturating_sub(margin),
        y0.saturating_sub(margin),
        (x1 + margin).min(cols.saturating_sub(1)),
        (y1 + margin).min(rows.saturating_sub(1)),
    ));
    options.bank_io_standards = per_bank.into_iter().collect();
    let fabric = db.load(&options).map_err(|e| e.to_string())?;
    if let Some(idcode) = s.device.idcode {
        fabric.check_idcode(idcode).map_err(|e| e.to_string())?;
    }
    Ok(fabric.arch)
}

pub fn run(s: &Synthesised, routers: &[Router]) -> Result<Vec<Checked>, String> {
    let (design, top, device, constraints) = (&s.design, s.top, &s.device, &s.constraints);
    let (arch, graph) = if device.family == "gowin" {
        let arch = gowin_arch(s)?;
        let graph = arch.build_graph();
        (arch, graph)
    } else {
        let arch = reticle::fpga::arch::architecture_for(&device.name)
            .ok_or_else(|| format!("no architecture for `{}`", device.name))?
            .clone();
        let graph = arch.build_graph();
        (arch, graph)
    };
    let netlist = Netlist::build(design, top, device, &graph).map_err(|e| e.to_string())?;
    let mut place_options = place::PlaceOptions::default();
    if let Some(seed) = crate::place_seed() {
        place_options.seed = seed;
    }
    let placed = std::time::Instant::now();
    let (placement, _) = place::place(&netlist, &arch, &graph, constraints, &place_options)
        .map_err(|e| e.to_string())?;
    let place_seconds = placed.elapsed().as_secs_f64();
    let route_options = RouteOptions::default();
    let units = Timing::for_netlist(&netlist, vec![1; graph.nodes.len()], 0);
    let mut results = Vec::new();
    for router in routers {
        let mut checked = Checked::new(router, &graph, &netlist);
        checked.place_seconds = place_seconds;
        let (routing, report) =
            match run_router(router, &netlist, &graph, &placement, &route_options) {
                Ok(done) => done,
                Err(error) => {
                    checked.failed = Some(error);
                    results.push(checked);
                    continue;
                }
            };
        checked.report = report;
        if let Some(error) = checked.report.replay_error.take() {
            checked.problems.push(error);
            results.push(checked);
            continue;
        }
        crate::measure(&mut checked, &graph, &routing);
        checked.critical = unit_longest(&netlist, &graph, &placement, &routing, &units);
        let problems = routing.verify(&netlist, &graph, &placement);
        let shorts = routing.shorts(&netlist, &graph);
        if !problems.is_empty() {
            checked.problems.push(format!(
                "verify: {} problem(s): {}",
                problems.len(),
                problems
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        } else if !shorts.is_empty() {
            checked.problems.push(format!("shorts: {}", shorts.len()));
        } else {
            match bitstream::generate(design, top, &arch, &graph, &netlist, &placement, &routing) {
                Ok(_) => checked
                    .notes
                    .push("verified, no shorts, and the bitstream's tiles generate".into()),
                Err(e) => checked.problems.push(format!("bitstream: {e}")),
            }
        }
        results.push(checked);
    }
    Ok(results)
}
