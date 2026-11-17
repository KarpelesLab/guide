//! A Xilinx 7-series design, by Project X-Ray's database, as `reticle fpga --bitstream` takes
//! it (`write_xc7_bitstream` in `src/bin/reticle/main.rs`) with the command line's defaults:
//! the same region, packing, placement and routing options. The one difference is that a
//! placement whose route does not converge is not placed again with the next seed
//! (`--route-attempts`), since the routers are compared on one placement.
//!
//! After routing, each router's result goes through everything the command line does after
//! its own: the lookup tables' inputs moved, `Routing::verify`, `Routing::shorts`, the clocks'
//! use of the global network, the bitstream, its decoding back through the database, and
//! the check that the arcs the bits select are exactly the ones routed.

use reticle::fpga::place::place_keeping_rules;
use reticle::fpga::route::RouteOptions;
use reticle::fpga::xray::{GridRegion, XrayDatabase, XrayOptions};
use reticle::fpga::{Netlist, Routing, bitstream};

use crate::front::{DiskFiles, Synthesised, database};
use crate::{Checked, Router, run_router};

fn needs_global_buffer(
    design: &reticle::ir::Design,
    top: reticle::ir::ModuleId,
    device: &reticle::fpga::Device,
) -> bool {
    use reticle::fpga::BelRole;
    use reticle::ir::CellKind;
    let buffers: Vec<&str> = device
        .bels
        .iter()
        .filter(|b| {
            matches!(
                b.role,
                BelRole::GlobalBuffer | BelRole::HorizontalBuffer | BelRole::RegionalBuffer
            )
        })
        .map(|b| b.name.as_str())
        .collect();
    let module = &design.modules[top];
    if module.cells.iter().any(|(_, cell)| {
        matches!(&cell.kind, CellKind::Blackbox(name) if buffers.contains(&name.as_str()))
    }) {
        return true;
    }
    let managers: Vec<reticle::ir::NetId> = module
        .cells
        .iter()
        .filter(|(_, cell)| {
            matches!(&cell.kind, CellKind::Blackbox(name)
                if device.bel_named(name.as_str()).is_some_and(|b| b.role == BelRole::Mmcm))
        })
        .flat_map(|(_, cell)| cell.outputs.iter().map(|(_, net)| *net))
        .collect();
    module.cells.iter().any(|(_, cell)| {
        let is_flop = matches!(&cell.kind, CellKind::Dff { .. })
            || matches!(&cell.kind, CellKind::Blackbox(name)
                if device.bel_named(name.as_str()).is_some_and(|b| b.role == BelRole::Ff));
        is_flop
            && ["clk", "C"].iter().any(|port| {
                cell.input(port)
                    .and_then(|e| module.exprs.get(e))
                    .and_then(reticle::ir::Expr::as_net)
                    .is_some_and(|net| managers.contains(&net))
            })
    })
}

fn needs_block_ram(
    design: &reticle::ir::Design,
    top: reticle::ir::ModuleId,
    device: &reticle::fpga::Device,
) -> bool {
    use reticle::ir::CellKind;
    design.modules[top].cells.iter().any(|(_, cell)| {
        matches!(&cell.kind, CellKind::Blackbox(name)
            if device.block_rams.iter().any(|b| b.name == name.as_str()))
    })
}

fn count_dsps(design: &reticle::ir::Design, top: reticle::ir::ModuleId) -> usize {
    use reticle::ir::CellKind;
    design.modules[top]
        .cells
        .iter()
        .filter(|(_, cell)| {
            matches!(&cell.kind, CellKind::Blackbox(name)
                if name.as_str() == reticle::fpga::xray::DSP_PRIMITIVE)
        })
        .count()
}

fn needs_clock_manager(
    design: &reticle::ir::Design,
    top: reticle::ir::ModuleId,
    device: &reticle::fpga::Device,
) -> bool {
    use reticle::ir::CellKind;
    let plls: Vec<&str> = device
        .clock_resources
        .plls
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    design.modules[top].cells.iter().any(|(_, cell)| {
        matches!(&cell.kind, CellKind::Blackbox(name)
            if plls.contains(&name.as_str())
                || device.bel_named(name.as_str()).is_some_and(|b| b.role == reticle::fpga::BelRole::Pll))
    })
}

fn needs_role(
    design: &reticle::ir::Design,
    top: reticle::ir::ModuleId,
    device: &reticle::fpga::Device,
    role: reticle::fpga::BelRole,
) -> bool {
    use reticle::ir::CellKind;
    design.modules[top].cells.iter().any(|(_, cell)| {
        matches!(&cell.kind, CellKind::Blackbox(name)
            if device.bel_named(name.as_str()).is_some_and(|b| b.role == role))
    })
}

fn needs_config_site(
    design: &reticle::ir::Design,
    top: reticle::ir::ModuleId,
    device: &reticle::fpga::Device,
) -> bool {
    use reticle::fpga::BelRole::{Bscan, Capture, Dna, Efuse, FrameEcc, Icap, Startup, UsrAccess};
    [
        Bscan, Capture, Dna, Efuse, FrameEcc, Icap, Startup, UsrAccess,
    ]
    .into_iter()
    .any(|role| needs_role(design, top, device, role))
}

/// Places the design, then routes and checks it with each router.
pub fn run(s: &Synthesised, routers: &[Router]) -> Result<Vec<Checked>, String> {
    let device = &s.device;
    let top = s.top;
    let constraints = &s.constraints;
    let mut bit_options = reticle::fpga::xc7::options::BitstreamOptions::default();
    for (name, value, _) in &constraints.bitstream_options {
        bit_options
            .set(name, value)
            .map_err(|e| format!("the constraints' `{name} {value}`: {e}"))?;
    }
    bit_options.check_options().map_err(|e| e.to_string())?;
    bit_options.resolve_timestamp(2026, 1, 1, (0, 0, 0));

    let root = database("RETICLE_CHIPDB", "prjxray-db")?;
    let files = DiskFiles::for_sources(&[]);
    let mut renamed = s.design.clone();
    reticle::fpga::xray::regional_buffers(
        &mut renamed,
        top,
        reticle::fpga::xray::RegionalClocks::Auto,
    );
    let design = &renamed;
    let mut options = XrayOptions::new().for_design(design, top, device);
    options.route_throughs = reticle::fpga::xray::RouteThroughMode::from_name("all")
        .ok_or("no route-through mode `all`")?;
    let db =
        XrayDatabase::open(&files, &root, &device.name, &options).map_err(|e| e.to_string())?;

    let resolved_regions;
    let constraints = if constraints.site_regions.is_empty() {
        constraints
    } else {
        let tiles = db.tiles(&files).map_err(|e| e.to_string())?;
        let mut owned = constraints.clone();
        let mut said = reticle::diag::Diagnostics::new();
        owned.resolve_site_regions(
            |site| {
                tiles
                    .iter()
                    .find(|t| t.sites.iter().any(|(name, _)| name == site))
                    .map(|t| (t.grid_x, t.grid_y))
            },
            &mut said,
        );
        resolved_regions = owned;
        &resolved_regions
    };

    // The region `reticle fpga` loads with no `--region`: see `write_xc7_bitstream`.
    let pins: Vec<String> = constraints.pins.iter().map(|p| p.pin.clone()).collect();
    options.region = db
        .region_for_pins(&files, &pins, 12)
        .map_err(|e| e.to_string())?;
    if options.region.is_none() {
        return Err("no constrained pin names a site this database has".to_owned());
    }
    for region in &constraints.regions {
        if let Some(loaded) = options.region {
            options.region = Some(GridRegion::new(
                loaded.x0.min(region.x0),
                loaded.y0.min(region.y0),
                loaded.x1.max(region.x1),
                loaded.y1.max(region.y1),
            ));
        }
    }
    if let Some(region) = options.region
        && needs_global_buffer(design, top, device)
    {
        options.region = db
            .region_with_site_type(&files, region, "BUFGCTRL")
            .map_err(|e| e.to_string())?
            .map(|r| GridRegion::new(r.x0.saturating_sub(2), r.y0.saturating_sub(4), r.x1, r.y1))
            .or(Some(region));
    }
    if let Some(region) = options.region
        && needs_clock_manager(design, top, device)
    {
        options.region = db
            .region_with_site_type(&files, region, "PLLE2_ADV")
            .map_err(|e| e.to_string())?
            .or(Some(region));
    }
    if let Some(region) = options.region
        && needs_config_site(design, top, device)
    {
        options.region = db
            .region_with_site_type(&files, region, "BSCAN")
            .map_err(|e| e.to_string())?
            .map(|r| GridRegion::new(r.x0, r.y0.saturating_sub(21), r.x1 + 4, r.y1 + 34))
            .or(Some(region));
    }
    for (role, site_type) in [
        (reticle::fpga::BelRole::Mmcm, "MMCME2_ADV"),
        (reticle::fpga::BelRole::Sysmon, "XADC"),
        (reticle::fpga::BelRole::DciReset, "DCIRESET"),
        (reticle::fpga::BelRole::HorizontalBuffer, "BUFHCE"),
        (reticle::fpga::BelRole::RegionalBuffer, "BUFR"),
        (reticle::fpga::BelRole::IoClockBuffer, "BUFIO"),
        (reticle::fpga::BelRole::MultiRegionBuffer, "BUFMRCE"),
    ] {
        if let Some(region) = options.region
            && needs_role(design, top, device, role)
        {
            options.region = db
                .region_with_site_type(&files, region, site_type)
                .map_err(|e| e.to_string())?
                .map(|r| {
                    if role == reticle::fpga::BelRole::MultiRegionBuffer {
                        GridRegion::new(r.x0, r.y0.saturating_sub(9), r.x1, r.y1 + 6)
                    } else if role == reticle::fpga::BelRole::Sysmon {
                        GridRegion::new(r.x0, r.y0.saturating_sub(2), r.x1 + 3, r.y1)
                    } else {
                        r
                    }
                })
                .or(Some(region));
        }
    }
    if let Some(region) = options.region
        && needs_block_ram(design, top, device)
    {
        options.region = db
            .region_with_site_type(&files, region, "RAMB18E1")
            .map_err(|e| e.to_string())?
            .map(|r| GridRegion::new(r.x0, r.y0.saturating_sub(6), r.x1 + 2, r.y1 + 6))
            .or(Some(region));
    }
    let dsps = count_dsps(design, top);
    if dsps > 0
        && let Some(region) = options.region
    {
        options.region = db
            .region_with_sites(&files, region, reticle::fpga::xray::DSP_PRIMITIVE, dsps + 4)
            .map_err(|e| e.to_string())?
            .map(|r| {
                GridRegion::new(
                    r.x0.saturating_sub(2),
                    r.y0.saturating_sub(6),
                    r.x1 + 2,
                    r.y1 + 6,
                )
            })
            .or(Some(region));
    }
    if let Some(region) = options.region
        && needs_global_buffer(design, top, device)
    {
        options.region = Some(
            db.region_with_clock_rows(&files, region)
                .map_err(|e| e.to_string())?,
        );
    }
    if let Some(region) = options.region
        && (needs_role(design, top, device, reticle::fpga::BelRole::IoDelay)
            || needs_role(design, top, device, reticle::fpga::BelRole::IoDelayControl))
    {
        let rows = db
            .region_with_clock_rows(&files, region)
            .map_err(|e| e.to_string())?;
        options.region = Some(GridRegion::new(
            rows.x0,
            rows.y0.saturating_sub(3),
            rows.x1,
            rows.y1 + 3,
        ));
    }

    let fabric = db.load(&files, &options).map_err(|e| e.to_string())?;
    if let Some(idcode) = device.idcode {
        fabric.check_idcode(idcode).map_err(|e| e.to_string())?;
    }
    let mut design = design.clone();
    let carry_options = reticle::fpga::xray::CarryOptions {
        generates_on_their_tables: true,
        generates_on_the_bypass: true,
        buffers_on_a6: true,
    };
    reticle::fpga::xray::legalise_carries_with(&mut design, top, device, &carry_options)?;
    let graph = fabric.arch.build_graph();
    let delay_model = reticle::fpga::xray::DelayModel::default();
    let lut_sites = graph
        .site_counts()
        .into_iter()
        .find(|(kind, _)| kind == "lut")
        .map_or(0, |(_, n)| n);
    let pair = reticle::fpga::xray::PairOptions {
        min_shared: 2,
        spare_deep: (delay_model == reticle::fpga::xray::DelayModel::Vivado)
            .then_some(reticle::fpga::xray::SPARE_DEEP_PERCENT),
        budget: (delay_model == reticle::fpga::xray::DelayModel::Vivado)
            .then_some(lut_sites / 4 * 3),
    };
    reticle::fpga::xray::pack_lut_pairs(&mut design, top, device, &pair)?;
    let delays =
        reticle::fpga::xray::io_delay_controllers(&mut design, top, device, &fabric, &graph)?;
    reticle::fpga::xray::unread_dci_resets(&mut design.modules[top])?;
    let design = &design;

    let mut netlist = Netlist::build(design, top, device, &graph).map_err(|e| e.to_string())?;
    fabric.spread_block_ram_pins(design, top, &mut netlist);
    reticle::fpga::xray::check_regional_wiring(&netlist)?;
    reticle::fpga::xray::check_serdes_wiring(&netlist)?;
    reticle::fpga::xray::tie_bypass_generates(design, top, &mut netlist);
    let mut place = reticle::fpga::place::PlaceOptions {
        move_effort: reticle::fpga::xray::PLACE_EFFORT,
        ..reticle::fpga::place::PlaceOptions::default()
    };
    if let Some(seed) = crate::place_seed() {
        place.seed = seed;
    }
    place.flat_steps = reticle::fpga::xray::PLACE_FLAT_STEPS;
    place.flat_fraction = reticle::fpga::xray::PLACE_FLAT_FRACTION;
    place.clock_roads = fabric.clock_roads(&graph);
    place.clock_regions = fabric.clock_regions();
    place.clock_region_lines = reticle::fpga::xray::CLOCK_REGION_LINES;
    place.regional_sites = fabric.regional_sites(&graph);
    place.memory_packing = true;
    place.route_throughs = fabric.route_throughs(design, top, &netlist, &graph);
    let grade = reticle::fpga::xray::SpeedGrade::from_name(&device.speed_grade)
        .unwrap_or(reticle::fpga::xray::SpeedGrade::One);
    place.median_moves = delay_model == reticle::fpga::xray::DelayModel::Vivado;

    let network_signals: Vec<bool> = netlist
        .signals
        .iter()
        .map(|signal| {
            signal.driver.is_some_and(|pin| {
                let pin = &netlist.pins[pin];
                match netlist.instances[pin.instance].kind.as_str() {
                    "gb" | "bufh" | "bufr" | "bufio" => true,
                    "pll" | "mmcm" => pin.role.starts_with("out") || pin.role.starts_with("fbout"),
                    _ => false,
                }
            }) || signal.sinks.iter().any(|sink| {
                let pin = &netlist.pins[*sink];
                pin.role == "i" && netlist.instances[pin.instance].kind == "bufh"
            })
        })
        .collect();
    let network_pins: Vec<bool> = netlist
        .pins
        .iter()
        .map(|pin| {
            let clock_pin =
                reticle::fpga::xray::clock_pin_roles(&netlist.instances[pin.instance].kind)
                    .contains(&pin.role.as_str());
            clock_pin
                && pin
                    .signal
                    .and_then(|s| netlist.signals[s].driver)
                    .is_some_and(|d| {
                        matches!(
                            netlist.instances[netlist.pins[d].instance].kind.as_str(),
                            "gb" | "bufh" | "bufr" | "bufio"
                        )
                    })
        })
        .collect();

    // The timing model the command line's `--timing-driven` places and routes by: built
    // always, to time every finished route alike, and given to the placer and to reticle's
    // router only with `--timing-driven`.
    let (mut timing, domains) =
        reticle::fpga::xray::place_timing(design, top, &netlist, constraints, delay_model, grade);
    if delay_model == reticle::fpga::xray::DelayModel::Vivado {
        timing.clock =
            reticle::fpga::xray::clock_arrival(&fabric.part.name, grade, &fabric.arch, &graph);
        if !timing.clock.is_empty() && !place.clock_regions.is_empty() {
            let width = fabric.arch.width;
            timing.clock_region = graph
                .sites
                .iter()
                .map(|site| {
                    let (x, y) = site.tile;
                    place
                        .clock_regions
                        .get((y * width + x) as usize)
                        .copied()
                        .unwrap_or(reticle::fpga::place::NO_CLOCK_REGION)
                })
                .collect();
        }
        timing.bel_launch = reticle::fpga::xray::bel_launch(delay_model, grade, &graph);
    }
    let timing_driven = crate::timing_driven();
    if timing_driven {
        place.timing = Some(timing.clone());
    }
    let node_delay = db
        .node_delays(&files, &fabric.arch, &graph, delay_model, grade)
        .map_err(|e| e.to_string())?;

    let placed = std::time::Instant::now();
    let (mut placement, placement_report, _rules) =
        place_keeping_rules(&netlist, &fabric.arch, &graph, constraints, &place)
            .map_err(|e| e.to_string())?;
    let place_seconds = placed.elapsed().as_secs_f64();
    let needed = placement_report.route_throughs_needed.clone();
    timing.local.clone_from(&placement_report.local);
    timing.skew_from(&placement);
    reticle::fpga::xray::place_timing_on_sites(
        &mut timing,
        delay_model,
        grade,
        &netlist,
        &graph,
        &placement,
        &needed,
    );
    fabric.place_io_delay_controllers(&delays, &netlist, &graph, &mut placement)?;

    let mut node_base = fabric.node_costs(&graph, reticle::fpga::xray::CLOCK_PREFERENCE);
    let mut network = fabric.clock_network_nodes(&graph);
    let ties = fabric
        .block_ram_ground_fans(design, top, &graph, &netlist, &placement)
        .map_err(|e| e.to_string())?;
    let mut closed = Vec::new();
    ties.close(graph.nodes.len(), &mut node_base, &mut network, &mut closed);
    fabric.unread_lane_ties(&graph, &netlist, &placement).close(
        graph.nodes.len(),
        &mut node_base,
        &mut network,
        &mut closed,
    );
    fabric.bypass_tie_wires(&graph, &netlist, &placement).close(
        graph.nodes.len(),
        &mut node_base,
        &mut network,
        &mut closed,
    );
    let mut equivalent =
        reticle::fpga::xray::lut_input_classes(design, top, &netlist, &graph, &placement);
    fabric.route_through_nodes(&graph, &needed, &mut closed, &mut equivalent);
    let route_options = RouteOptions {
        node_base,
        network,
        closed,
        network_signals,
        network_entry: fabric.clock_pin_entries(&graph),
        network_pins,
        first: netlist
            .pins
            .iter()
            .map(|pin| {
                let from_a_pad = pin
                    .signal
                    .and_then(|signal| netlist.signals[signal].driver)
                    .is_some_and(|driver| {
                        netlist.instances[netlist.pins[driver].instance].kind == "io"
                    });
                from_a_pad
                    && matches!(
                        netlist.instances[pin.instance].kind.as_str(),
                        "gb" | "bufh" | "bufr" | "bufio"
                    )
                    && (pin.role == "i" || pin.role == "i1")
            })
            .collect(),
        astar_weight: reticle::fpga::xray::ASTAR_WEIGHT,
        reroute_congested_only: true,
        incremental: true,
        first_pass_present: Some(0.0),
        bounding_margin: Some(3),
        present_growth: reticle::fpga::xray::PRESENT_GROWTH,
        uncritical_present_cap: reticle::fpga::xray::UNCRITICAL_PRESENT_CAP,
        uncritical_cap_until: reticle::fpga::xray::UNCRITICAL_CAP_UNTIL,
        max_iterations: reticle::fpga::xray::ROUTE_PASSES,
        equivalent,
        timing: timing_driven.then(|| reticle::fpga::route::Timing {
            node_delay: node_delay.clone(),
            cell_delay: 0,
            combinational: Vec::new(),
            clock_roles: Vec::new(),
            max_criticality: 0.99,
            cleanups: 2,
            tile_delay: reticle::fpga::xray::route_tile_delay_ps(grade),
            exponent: reticle::fpga::xray::FITTED_CRITICALITY_EXPONENT,
            passes: 4,
            reroute_above: reticle::fpga::xray::FITTED_REROUTE_ABOVE,
            paths: Some(timing.clone()),
            present_cap: reticle::fpga::xray::CRITICAL_PRESENT_CAP,
            present_cap_run: reticle::fpga::xray::CRITICAL_CAP_RUN,
            search_budget: reticle::fpga::route::CRITICAL_SEARCH_BUDGET,
            branch: reticle::fpga::xray::delay::branch_load(grade),
            claim_inputs: reticle::fpga::xray::FITTED_CLAIM_INPUTS,
            critical_astar: reticle::fpga::xray::CRITICAL_ASTAR_WEIGHT,
            seed_from_placement: true,
            delay_weight: reticle::fpga::xray::FITTED_DELAY_WEIGHT,
            keep_criticality: true,
            refine_rounds: reticle::fpga::xray::FITTED_REFINE_ROUNDS,
            refine_above: reticle::fpga::xray::FITTED_REFINE_ABOVE,
            against_longest: true,
        }),
        ..RouteOptions::default()
    };

    let mut results = Vec::new();
    for router in routers {
        let mut checked = Checked::new(router, &graph, &netlist);
        checked.place_seconds = place_seconds;
        let routed = run_router(router, &netlist, &graph, &placement, &route_options);
        let (routing, report) = match routed {
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
        // The routed timing, by the model `--timing-driven` routes by.
        let wire = reticle::fpga::route::connection_delays(
            &netlist,
            &graph,
            &placement,
            &routing,
            &node_delay,
            reticle::fpga::xray::delay::branch_load(grade),
        );
        let (_, longest) = reticle::fpga::place::path_criticality(&netlist, &timing, &wire);
        // The model guide routes by, against reticle's own analysis, on these routes: with
        // no branch load, which guide's model leaves out, and with it.
        if crate::timing_driven() {
            let bare = reticle::fpga::route::connection_delays(
                &netlist,
                &graph,
                &placement,
                &routing,
                &node_delay,
                reticle::fpga::route::BranchLoad::NONE,
            );
            let (_, bare_longest) =
                reticle::fpga::place::path_criticality(&netlist, &timing, &bare);
            match crate::timed::latest(&netlist, &timing, &bare) {
                Ok((arrival, slack)) => eprintln!(
                    "note: {}: reticle's analysis {longest:?} ps, without branch loads {bare_longest:?}; guide's model without them: arrival {arrival} ps, worst slack {slack:?} ps",
                    router.name()
                ),
                Err(e) => eprintln!("note: guide's model: {e}"),
            }
        }
        checked.critical = domains
            .iter()
            .zip(&longest)
            .map(|(name, ps)| (name.clone(), *ps))
            .collect();
        match check(
            design,
            top,
            device,
            constraints,
            &netlist,
            &graph,
            &placement,
            &routing,
            &fabric,
            &db,
            &files,
            &bit_options,
        ) {
            Ok(notes) => checked.notes = notes,
            Err(error) => checked.problems.push(error),
        }
        results.push(checked);
    }
    Ok(results)
}

/// What the command line does with a routing before it writes a bitstream, and the checks it
/// refuses to write one without.
#[allow(clippy::too_many_arguments)]
fn check(
    design: &reticle::ir::Design,
    top: reticle::ir::ModuleId,
    device: &reticle::fpga::Device,
    constraints: &reticle::fpga::Constraints,
    netlist: &Netlist,
    graph: &reticle::fpga::arch::RoutingGraph,
    placement: &reticle::fpga::place::Placement,
    routing: &Routing,
    fabric: &reticle::fpga::xray::XrayFabric,
    db: &XrayDatabase,
    files: &DiskFiles,
    bit_options: &reticle::fpga::xc7::options::BitstreamOptions,
) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    let mut netlist = netlist.clone();
    let mut swapped = design.clone();
    let tables = reticle::fpga::xray::apply_lut_moves(
        &mut swapped,
        top,
        &mut netlist,
        graph,
        placement,
        routing,
    )?;
    notes.push(format!(
        "{tables} lookup table(s) take their inputs on other pins"
    ));
    let design = &swapped;
    let netlist = &netlist;
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
    let clocks = fabric.clock_network_use(netlist, placement, graph, routing);
    if !clocks.off_network.is_empty() {
        return Err(format!(
            "{} clock pin(s) off the global network: {}",
            clocks.off_network.len(),
            clocks
                .off_network
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
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
    fabric.configure_route_throughs(graph, placement, routing, &mut tiles)?;
    fabric
        .enable_global_clocks(graph, routing, &mut tiles)
        .map_err(|e| e.to_string())?;
    fabric
        .configure_io(design, top, graph, netlist, placement, &mut tiles)
        .map_err(|e| e.to_string())?;
    fabric
        .configure_clock_managers(
            design,
            top,
            graph,
            netlist,
            placement,
            &constraints
                .clocks
                .iter()
                .map(|c| (c.net.clone(), c.period_ns))
                .collect(),
            &mut tiles,
        )
        .map_err(|e| e.to_string())?;
    fabric
        .configure_global_buffers(design, top, graph, netlist, placement, &mut tiles)
        .map_err(|e| e.to_string())?;
    fabric
        .configure_regional_buffers(design, top, graph, netlist, placement, routing, &mut tiles)
        .map_err(|e| e.to_string())?;
    fabric
        .configure_io_logic(design, top, graph, netlist, placement, &mut tiles)
        .map_err(|e| e.to_string())?;
    let config_use = fabric
        .configure_config(design, top, graph, netlist, placement, &mut tiles)
        .map_err(|e| e.to_string())?;
    let mut bit_options = bit_options.clone();
    bit_options.set_design(config_use);
    bit_options.check().map_err(|e| e.to_string())?;
    fabric
        .configure_io_delays(design, top, graph, netlist, placement, &mut tiles)
        .map_err(|e| e.to_string())?;
    let block_rams = fabric
        .configure_block_rams(design, top, graph, netlist, placement, routing, &mut tiles)
        .map_err(|e| e.to_string())?;
    let mut frames =
        reticle::fpga::xc7::frames_from_bitstream(&fabric.part, &tiles, &fabric.frames)
            .map_err(|e| e.to_string())?;
    db.finish_io_banks(
        files,
        &mut frames,
        &fabric.io_banks(design, top, graph, netlist, placement),
    )
    .map_err(|e| e.to_string())?;
    db.finish_frames(files, &mut frames, &bit_options)
        .map_err(|e| e.to_string())?;
    let dynamic = fabric.dynamic_table_bits(graph, netlist, placement, &tiles);
    let frames = frames.with_ecc_without(&dynamic);
    let decoded = db.decode(files, &frames).map_err(|e| e.to_string())?;
    if decoded.unexplained > 0 {
        return Err(format!("{} set bit(s) unexplained", decoded.unexplained));
    }
    let (extra, lost) = db
        .arc_differences(
            files,
            &fabric.arch,
            graph,
            routing,
            &block_rams.tie_pips,
            &decoded,
        )
        .map_err(|e| e.to_string())?;
    if !extra.is_empty() || !lost.is_empty() {
        return Err(format!(
            "the bits do not select the arcs routed: {} selected and not chosen, {} chosen and not selected",
            extra.len(),
            lost.len()
        ));
    }
    db.check_route_throughs(
        &db.tiles(files).map_err(|e| e.to_string())?,
        graph,
        design,
        top,
        netlist,
        placement,
        routing,
        &decoded,
    )
    .map_err(|e| format!("a lookup table does not decode as placed and routed: {e}"))?;
    let _ = device;
    notes.push(format!(
        "every bit decodes: {} bit(s) set, 0 unexplained, and the arcs they select are exactly the ones routed",
        frames.ones()
    ));
    Ok(notes)
}
