//! The Xilinx 7-series flow of `reticle fpga --bitstream`, from Project X-Ray's database,
//! with the placement step open: what `write_xc7_bitstream` in reticle's command-line tool
//! does before and after placement, at its defaults (`--delay-model vivado`, not
//! `--timing-driven`, one route attempt), up to the bitstream, which is not written.
//!
//! The timing of each routed placement is analysed as a `--timing-driven` build reports it:
//! each domain's latest path, from the route's own connection delays, by reticle's fitted
//! delay model.

use reticle::fpga::arch::{NodeId, RoutingGraph};
use reticle::fpga::place::{Netlist, PlaceOptions, PlaceTiming, Placement};
use reticle::fpga::route::{RouteOptions, Routing};
use reticle::fpga::xray::{
    self, DelayModel, GridRegion, SpeedGrade, XrayDatabase, XrayFabric, XrayOptions,
};
use reticle::fpga::{BelRole, Constraints, Device};
use reticle::ir::{CellKind, Design, ModuleId};

use crate::flow::Loaded;
use crate::front::DiskFiles;

/// The database, the fabric, and the design as the flow changed it before placement.
pub struct Xc7 {
    db: XrayDatabase,
    /// The fabric.
    pub fabric: XrayFabric,
    /// The design, with its carries legalised and its lookup tables paired.
    pub design: Design,
    delays: xray::IoDelayPlan,
    model: DelayModel,
    grade: SpeedGrade,
    node_delay: Vec<u32>,
}

fn database_root() -> Result<String, String> {
    if let Ok(root) = std::env::var("RETICLE_CHIPDB") {
        return Ok(root);
    }
    crate::flow::cached("prjxray-db")
}

fn blackbox_named(design: &Design, top: ModuleId, wanted: impl Fn(&str) -> bool) -> bool {
    design.modules[top]
        .cells
        .iter()
        .any(|(_, cell)| matches!(&cell.kind, CellKind::Blackbox(name) if wanted(name.as_str())))
}

fn needs_role(design: &Design, top: ModuleId, device: &Device, role: BelRole) -> bool {
    blackbox_named(design, top, |name| {
        device.bel_named(name).is_some_and(|b| b.role == role)
    })
}

fn needs_global_buffer(design: &Design, top: ModuleId, device: &Device) -> bool {
    blackbox_named(design, top, |name| {
        device.bels.iter().any(|b| {
            b.name == name
                && matches!(
                    b.role,
                    BelRole::GlobalBuffer | BelRole::HorizontalBuffer | BelRole::RegionalBuffer
                )
        })
    })
}

/// Opens the database, loads the fabric around the pins as `reticle fpga` does, and runs
/// the 7-series steps before placement.
pub fn load(
    design: &Design,
    top: ModuleId,
    device: &Device,
    constraints: &Constraints,
) -> Result<(Xc7, Loaded), String> {
    let files = DiskFiles::for_sources(&[]);
    let root = database_root()?;
    let mut renamed = design.clone();
    xray::regional_buffers(&mut renamed, top, xray::RegionalClocks::Auto);
    let design = &renamed;
    let mut options = XrayOptions::new().for_design(design, top, device);
    options.route_throughs = xray::RouteThroughMode::from_name("all").ok_or("route throughs")?;
    let db =
        XrayDatabase::open(&files, &root, &device.name, &options).map_err(|e| e.to_string())?;
    if !constraints.site_regions.is_empty() {
        return Err("pblocks are not supported by this harness".to_owned());
    }

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
    let grow = |options: &mut XrayOptions,
                site_type: &str,
                adjust: &dyn Fn(GridRegion) -> GridRegion|
     -> Result<(), String> {
        if let Some(region) = options.region {
            options.region = db
                .region_with_site_type(&files, region, site_type)
                .map_err(|e| e.to_string())?
                .map(adjust)
                .or(Some(region));
        }
        Ok(())
    };
    if needs_global_buffer(design, top, device) {
        grow(&mut options, "BUFGCTRL", &|r| {
            GridRegion::new(r.x0.saturating_sub(2), r.y0.saturating_sub(4), r.x1, r.y1)
        })?;
    }
    let plls: Vec<&str> = device
        .clock_resources
        .plls
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    if blackbox_named(design, top, |name| {
        plls.contains(&name)
            || device
                .bel_named(name)
                .is_some_and(|b| b.role == BelRole::Pll)
    }) {
        grow(&mut options, "PLLE2_ADV", &|r| r)?;
    }
    use BelRole::{Bscan, Capture, Dna, Efuse, FrameEcc, Icap, Startup, UsrAccess};
    if [
        Bscan, Capture, Dna, Efuse, FrameEcc, Icap, Startup, UsrAccess,
    ]
    .into_iter()
    .any(|role| needs_role(design, top, device, role))
    {
        grow(&mut options, "BSCAN", &|r| {
            GridRegion::new(r.x0, r.y0.saturating_sub(21), r.x1 + 4, r.y1 + 34)
        })?;
    }
    for (role, site_type) in [
        (BelRole::Mmcm, "MMCME2_ADV"),
        (BelRole::Sysmon, "XADC"),
        (BelRole::DciReset, "DCIRESET"),
        (BelRole::HorizontalBuffer, "BUFHCE"),
        (BelRole::RegionalBuffer, "BUFR"),
        (BelRole::IoClockBuffer, "BUFIO"),
        (BelRole::MultiRegionBuffer, "BUFMRCE"),
    ] {
        if needs_role(design, top, device, role) {
            grow(&mut options, site_type, &|r| {
                if role == BelRole::MultiRegionBuffer {
                    GridRegion::new(r.x0, r.y0.saturating_sub(9), r.x1, r.y1 + 6)
                } else if role == BelRole::Sysmon {
                    GridRegion::new(r.x0, r.y0.saturating_sub(2), r.x1 + 3, r.y1)
                } else {
                    r
                }
            })?;
        }
    }
    if blackbox_named(design, top, |name| {
        device.block_rams.iter().any(|b| b.name == name)
    }) {
        grow(&mut options, "RAMB18E1", &|r| {
            GridRegion::new(r.x0, r.y0.saturating_sub(6), r.x1 + 2, r.y1 + 6)
        })?;
    }
    let dsps = design.modules[top]
        .cells
        .iter()
        .filter(
            |(_, c)| matches!(&c.kind, CellKind::Blackbox(n) if n.as_str() == xray::DSP_PRIMITIVE),
        )
        .count();
    if dsps > 0
        && let Some(region) = options.region
    {
        options.region = db
            .region_with_sites(&files, region, xray::DSP_PRIMITIVE, dsps + 4)
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
        && (needs_role(design, top, device, BelRole::IoDelay)
            || needs_role(design, top, device, BelRole::IoDelayControl))
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
    let carry_options = xray::CarryOptions {
        generates_on_their_tables: true,
        generates_on_the_bypass: true,
        buffers_on_a6: true,
    };
    xray::legalise_carries_with(&mut design, top, device, &carry_options)
        .map_err(|e| e.to_string())?;
    let graph = fabric.arch.build_graph();
    let model = DelayModel::default();
    let lut_sites = graph
        .site_counts()
        .into_iter()
        .find(|(kind, _)| kind == "lut")
        .map_or(0, |(_, n)| n);
    let pair = xray::PairOptions {
        min_shared: 2,
        spare_deep: (model == DelayModel::Vivado).then_some(xray::SPARE_DEEP_PERCENT),
        budget: (model == DelayModel::Vivado).then_some(lut_sites / 4 * 3),
    };
    xray::pack_lut_pairs(&mut design, top, device, &pair).map_err(|e| e.to_string())?;
    let delays = xray::io_delay_controllers(&mut design, top, device, &fabric, &graph)
        .map_err(|e| e.to_string())?;
    xray::unread_dci_resets(&mut design.modules[top]).map_err(|e| e.to_string())?;
    let mut netlist = Netlist::build(&design, top, device, &graph).map_err(|e| e.to_string())?;
    fabric.spread_block_ram_pins(&design, top, &mut netlist);
    crate::flow::check_driverless(&netlist)?;
    xray::check_regional_wiring(&netlist).map_err(|e| e.to_string())?;
    xray::check_serdes_wiring(&netlist).map_err(|e| e.to_string())?;
    xray::tie_bypass_generates(&design, top, &mut netlist);
    let grade = SpeedGrade::from_name(&device.speed_grade).unwrap_or(SpeedGrade::Three);
    let node_delay = db
        .node_delays(&files, &fabric.arch, &graph, model, grade)
        .map_err(|e| e.to_string())?;
    Ok((
        Xc7 {
            db,
            fabric,
            design,
            delays,
            model,
            grade,
            node_delay,
        },
        Loaded { graph, netlist },
    ))
}

impl Xc7 {
    /// The placer's options as `reticle fpga` sets them for this family.
    pub fn place_options(
        &self,
        graph: &RoutingGraph,
        netlist: &Netlist,
        top: ModuleId,
        seed: u64,
    ) -> PlaceOptions {
        let mut options = PlaceOptions {
            seed,
            ..PlaceOptions::default()
        };
        options.move_effort = xray::PLACE_EFFORT;
        options.flat_steps = xray::PLACE_FLAT_STEPS;
        options.flat_fraction = xray::PLACE_FLAT_FRACTION;
        options.clock_roads = self.fabric.clock_roads(graph);
        options.clock_regions = self.fabric.clock_regions();
        options.clock_region_lines = xray::CLOCK_REGION_LINES;
        options.regional_sites = self.fabric.regional_sites(graph);
        options.memory_packing = true;
        options.route_throughs = self
            .fabric
            .route_throughs(&self.design, top, netlist, graph);
        options.median_moves = self.model == DelayModel::Vivado;
        options
    }

    /// Seats the input delays' controllers, as the flow does after placement.
    pub fn after_placement(
        &self,
        graph: &RoutingGraph,
        netlist: &Netlist,
        placement: &mut Placement,
    ) -> Result<(), String> {
        self.fabric
            .place_io_delay_controllers(&self.delays, netlist, graph, placement)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The router's options as `reticle fpga` sets them, with only the ways through a lookup
    /// table that `needed` names open.
    pub fn route_options(
        &self,
        top: ModuleId,
        graph: &RoutingGraph,
        netlist: &Netlist,
        placement: &Placement,
        needed: &[NodeId],
    ) -> Result<RouteOptions, String> {
        let fabric = &self.fabric;
        let design = &self.design;
        let mut node_base = fabric.node_costs(graph, xray::CLOCK_PREFERENCE);
        let mut network = fabric.clock_network_nodes(graph);
        let ties = fabric
            .block_ram_ground_fans(design, top, graph, netlist, placement)
            .map_err(|e| e.to_string())?;
        let mut closed = Vec::new();
        ties.close(graph.nodes.len(), &mut node_base, &mut network, &mut closed);
        if self.model == DelayModel::Vivado {
            fabric.unread_lane_ties(graph, netlist, placement).close(
                graph.nodes.len(),
                &mut node_base,
                &mut network,
                &mut closed,
            );
        }
        fabric.bypass_tie_wires(graph, netlist, placement).close(
            graph.nodes.len(),
            &mut node_base,
            &mut network,
            &mut closed,
        );
        let mut equivalent = xray::lut_input_classes(design, top, netlist, graph, placement);
        fabric.route_through_nodes(graph, needed, &mut closed, &mut equivalent);
        let clock_driver = |kind: &str| matches!(kind, "gb" | "bufh" | "bufr" | "bufio");
        let network_signals: Vec<bool> = netlist
            .signals
            .iter()
            .map(|signal| {
                signal.driver.is_some_and(|pin| {
                    let pin = &netlist.pins[pin];
                    match netlist.instances[pin.instance].kind.as_str() {
                        k if clock_driver(k) => true,
                        "pll" | "mmcm" => {
                            pin.role.starts_with("out") || pin.role.starts_with("fbout")
                        }
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
                xray::clock_pin_roles(&netlist.instances[pin.instance].kind)
                    .contains(&pin.role.as_str())
                    && pin
                        .signal
                        .and_then(|s| netlist.signals[s].driver)
                        .is_some_and(|d| {
                            clock_driver(&netlist.instances[netlist.pins[d].instance].kind)
                        })
            })
            .collect();
        let first = netlist
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
                    && clock_driver(&netlist.instances[pin.instance].kind)
                    && (pin.role == "i" || pin.role == "i1")
            })
            .collect();
        Ok(RouteOptions {
            node_base,
            network,
            closed,
            network_signals,
            network_entry: fabric.clock_pin_entries(graph),
            network_pins,
            first,
            astar_weight: xray::ASTAR_WEIGHT,
            reroute_congested_only: true,
            incremental: true,
            first_pass_present: Some(0.0),
            bounding_margin: Some(3),
            present_growth: xray::PRESENT_GROWTH,
            uncritical_present_cap: xray::UNCRITICAL_PRESENT_CAP,
            uncritical_cap_until: xray::UNCRITICAL_CAP_UNTIL,
            max_iterations: xray::ROUTE_PASSES,
            equivalent,
            ..RouteOptions::default()
        })
    }

    /// The timing paths as a `--timing-driven` build sets them up, for analysis.
    pub fn timing(
        &self,
        top: ModuleId,
        graph: &RoutingGraph,
        netlist: &Netlist,
        constraints: &Constraints,
        clock_regions: &[u16],
    ) -> (PlaceTiming, Vec<String>) {
        let (mut timing, names) = xray::place_timing(
            &self.design,
            top,
            netlist,
            constraints,
            self.model,
            self.grade,
        );
        if self.model == DelayModel::Vivado {
            timing.clock =
                xray::clock_arrival(&self.fabric.part.name, self.grade, &self.fabric.arch, graph);
            if !timing.clock.is_empty() && !clock_regions.is_empty() {
                let width = self.fabric.arch.width;
                timing.clock_region = graph
                    .sites
                    .iter()
                    .map(|site| {
                        let (x, y) = site.tile;
                        clock_regions
                            .get((y * width + x) as usize)
                            .copied()
                            .unwrap_or(reticle::fpga::place::NO_CLOCK_REGION)
                    })
                    .collect();
            }
            timing.bel_launch = xray::bel_launch(self.model, self.grade, graph);
        }
        (timing, names)
    }

    /// Each domain's latest path through the routed placement, in picoseconds, with the
    /// periods: the flow's "routed timing estimate".
    pub fn routed_timing(
        &self,
        timing: &PlaceTiming,
        graph: &RoutingGraph,
        netlist: &Netlist,
        placement: &Placement,
        routing: &Routing,
        needed: &[NodeId],
    ) -> Vec<(u64, String)> {
        let mut timing = timing.clone();
        timing.skew_from(placement);
        xray::place_timing_on_sites(
            &mut timing,
            self.model,
            self.grade,
            netlist,
            graph,
            placement,
            needed,
        );
        let branch = match self.model {
            DelayModel::Vivado => xray::delay::branch_load(self.grade),
            DelayModel::Legacy => reticle::fpga::route::BranchLoad::NONE,
        };
        let wire = reticle::fpga::route::connection_delays(
            netlist,
            graph,
            placement,
            routing,
            &self.node_delay,
            branch,
        );
        let (_, longest) = reticle::fpga::place::path_criticality(netlist, &timing, &wire);
        longest
            .into_iter()
            .zip(timing.periods.iter().map(ToString::to_string))
            .collect()
    }

    /// The flow's verification of a route: lookup tables given the inputs the router reached
    /// them on, then every sink walked back to its driver, and no wire carrying two signals.
    pub fn verify(
        &self,
        top: ModuleId,
        graph: &RoutingGraph,
        netlist: &Netlist,
        placement: &Placement,
        routing: &Routing,
    ) -> Result<(), String> {
        let mut design = self.design.clone();
        let mut netlist = netlist.clone();
        xray::apply_lut_moves(&mut design, top, &mut netlist, graph, placement, routing)
            .map_err(|e| e.to_string())?;
        let problems = routing.verify(&netlist, graph, placement);
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let shorts = routing.shorts(&netlist, graph);
        if !shorts.is_empty() {
            return Err(format!(
                "{} wire(s) carry two signals: {}",
                shorts.len(),
                shorts.join("; ")
            ));
        }
        Ok(())
    }

    /// The checks the flow makes of a route: every clock pin a global buffer drives reached
    /// over the clock network.
    pub fn check(
        &self,
        graph: &RoutingGraph,
        netlist: &Netlist,
        placement: &Placement,
        routing: &Routing,
    ) -> Result<(), String> {
        let clocks = self
            .fabric
            .clock_network_use(netlist, placement, graph, routing);
        if !clocks.off_network.is_empty() {
            return Err(format!(
                "{} clock pin(s) off the global network: {}",
                clocks.off_network.len(),
                clocks.off_network.join(", ")
            ));
        }
        let _ = &self.db;
        Ok(())
    }
}
