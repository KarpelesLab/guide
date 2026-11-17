//! A Lattice ECP5 design, by Project Trellis' database, as `reticle fpga --bitstream` takes
//! it (`write_ecp5_bitstream` in `src/bin/reticle/main.rs`): the whole die, the command
//! line's placement, and routing that steers clocks onto the global network. Each router's
//! result then goes through what the command line does before it writes a bitstream: verify,
//! the cells configured, the clocks on the network, and the image decoded back through the
//! database into exactly the arcs routed.

use reticle::fpga::route::{RouteOptions, Timing};
use reticle::fpga::trellis::{self, TrellisOptions};
use reticle::fpga::{Netlist, Routing, bitstream, place};

use crate::front::{DiskFiles, Synthesised, database};
use crate::{Checked, Router, run_router};

const ECP5_PARTS: &[(&str, &str, &str, &str)] =
    &[("ecp5-12f-CABGA256", "LFE5U-12F", "CABGA256", "8")];

pub fn run(s: &Synthesised, routers: &[Router]) -> Result<Vec<Checked>, String> {
    let (design, top, device, constraints) = (&s.design, s.top, &s.device, &s.constraints);
    if let Some(message) = trellis::dsp_refusal(design, top) {
        return Err(message);
    }
    let Some(&(_, part, package, speed)) =
        ECP5_PARTS.iter().find(|(name, ..)| *name == device.name)
    else {
        return Err(format!("no Project Trellis part for `{}`", device.name));
    };
    let root = database("RETICLE_TRELLISDB", "prjtrellis-db")?;
    let db = trellis::open(&DiskFiles::for_sources(&[]), &root, part).map_err(|e| e.to_string())?;
    let mut options = TrellisOptions::new();
    options.package = package.to_owned();
    let mut standards: Vec<&str> = constraints
        .pins
        .iter()
        .filter_map(|p| p.io.io_standard.as_deref())
        .collect();
    standards.sort_unstable();
    standards.dedup();
    if let [one] = standards[..] {
        options.io_standard = one.to_owned();
    }
    let fabric = db.load(&options).map_err(|e| e.to_string())?;
    if let Some(idcode) = device.idcode {
        fabric.check_idcode(idcode).map_err(|e| e.to_string())?;
    }
    let graph = fabric.arch.build_graph();
    let netlist = Netlist::build(design, top, device, &graph).map_err(|e| e.to_string())?;
    let mut place_options = place::PlaceOptions::default();
    if let Some(seed) = crate::place_seed() {
        place_options.seed = seed;
    }
    let placed = std::time::Instant::now();
    let (placement, _) = place::place(&netlist, &fabric.arch, &graph, constraints, &place_options)
        .map_err(|e| e.to_string())?;
    let place_seconds = placed.elapsed().as_secs_f64();
    let route_options = RouteOptions {
        node_base: fabric.clock_node_costs(&graph, trellis::CLOCK_PREFERENCE),
        ..RouteOptions::default()
    };
    // No delay model for this family: the longest register-to-register path in graph nodes.
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
        checked.critical =
            crate::generic::unit_longest(&netlist, &graph, &placement, &routing, &units);
        match check(
            s, &netlist, &graph, &placement, &routing, &fabric, &db, speed,
        ) {
            Ok(notes) => checked.notes = notes,
            Err(error) => checked.problems.push(error),
        }
        results.push(checked);
    }
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
fn check(
    s: &Synthesised,
    netlist: &Netlist,
    graph: &reticle::fpga::arch::RoutingGraph,
    placement: &place::Placement,
    routing: &Routing,
    fabric: &trellis::TrellisFabric,
    db: &trellis::TrellisDatabase,
    speed: &str,
) -> Result<Vec<String>, String> {
    let (design, top, device) = (&s.design, s.top, &s.device);
    let problems = routing.verify(netlist, graph, placement);
    if !problems.is_empty() {
        return Err(format!(
            "verify: {} problem(s): {}",
            problems.len(),
            problems
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let shorts = routing.shorts(netlist, graph);
    if !shorts.is_empty() {
        return Err(format!("shorts: {}", shorts.len()));
    }
    let mut tiles = bitstream::generate(
        design,
        top,
        &fabric.arch,
        graph,
        netlist,
        placement,
        routing,
    )
    .map_err(|e| e.to_string())?;
    fabric
        .configure_io(design, top, netlist, placement, graph, &mut tiles)
        .map_err(|e| e.to_string())?;
    fabric
        .configure_logic(design, top, netlist, placement, graph, &mut tiles)
        .map_err(|e| e.to_string())?;
    fabric
        .configure_registers(design, top, netlist, placement, graph, routing, &mut tiles)
        .map_err(|e| e.to_string())?;
    let carries = fabric
        .configure_carry(design, top, netlist, placement, graph, &mut tiles)
        .map_err(|e| e.to_string())?;
    fabric
        .configure_lutram(design, top, netlist, placement, graph, &mut tiles)
        .map_err(|e| e.to_string())?;
    let brams = fabric
        .configure_bram(design, top, device, netlist, placement, graph, &mut tiles)
        .map_err(|e| e.to_string())?;
    if !brams.overlaps.is_empty() {
        return Err(format!("{} block RAM overlap(s)", brams.overlaps.len()));
    }
    let clocks = fabric.clock_network_use(netlist, placement, graph, routing);
    if !clocks.off_network.is_empty() {
        return Err(format!(
            "{} flip-flop clock(s) off the global network",
            clocks.off_network.len()
        ));
    }
    let mut stream = fabric.stream(&tiles, speed).map_err(|e| e.to_string())?;
    stream.bram = brams.blocks;
    let stolen = fabric.dropped_clear_bits(graph, routing, &tiles);
    if !stolen.is_empty() {
        return Err(format!("{} bit(s) wanted clear were set", stolen.len()));
    }
    let decoded = db.decode(&stream.cram);
    if decoded.unexplained > 0 {
        return Err(format!("{} set bit(s) unexplained", decoded.unexplained));
    }
    let (selected, unresolved) = db.resolved_arcs(&decoded);
    let mut chosen = fabric.routed_arcs(graph, routing);
    chosen.extend(carries.sums.iter().cloned());
    if !unresolved.is_empty() || selected != chosen {
        return Err(format!(
            "the bits select {} arc(s), the router chose {}, {} unresolved",
            selected.len(),
            chosen.len(),
            unresolved.len()
        ));
    }
    Ok(vec![format!(
        "every bit decodes: {} bit(s) set into {} arc(s), 0 unexplained, exactly the arcs routed",
        decoded.bits,
        decoded.arcs.len()
    )])
}
