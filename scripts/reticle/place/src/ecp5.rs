//! The Lattice ECP5 flow of `reticle fpga --bitstream`, from Project Trellis' database,
//! with the placement step open.

use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::place::{Netlist, Placement};
use reticle::fpga::trellis::{self, TrellisDatabase, TrellisFabric, TrellisOptions};
use reticle::fpga::{Constraints, Device, bitstream, route};
use reticle::ir::{Design, ModuleId};

use crate::flow::{Checked, Loaded};
use crate::front::DiskFiles;

/// The ECP5 parts reticle's flow knows: `(device, database part, package)`.
const PARTS: &[(&str, &str, &str)] = &[("ecp5-12f-CABGA256", "LFE5U-12F", "CABGA256")];

/// The database and the fabric, with the design's netlist on it.
pub struct Ecp5 {
    db: TrellisDatabase,
    /// The fabric.
    pub fabric: TrellisFabric,
}

/// Where Project Trellis' database is: `RETICLE_TRELLISDB`, or the copy `reticle fetch`
/// left in the cache.
fn database_root() -> Result<String, String> {
    if let Ok(root) = std::env::var("RETICLE_TRELLISDB") {
        return Ok(root);
    }
    crate::flow::cached("prjtrellis-db")
}

/// Opens the database and loads the fabric, the graph and the netlist.
pub fn load(
    design: &Design,
    top: ModuleId,
    device: &Device,
    constraints: &Constraints,
) -> Result<(Ecp5, Loaded), String> {
    if let Some(message) = trellis::dsp_refusal(design, top) {
        return Err(message);
    }
    let &(_, part, package) = PARTS
        .iter()
        .find(|(name, ..)| *name == device.name)
        .ok_or_else(|| format!("no Project Trellis part for `{}`", device.name))?;
    let root = database_root()?;
    let db = trellis::open(&DiskFiles::for_sources(&[]), &root, part).map_err(|e| e.to_string())?;
    let mut options = TrellisOptions::new();
    options.package = package.to_owned();
    if let Some(standard) = crate::flow::single_io_standard(constraints)? {
        options.io_standard = standard;
    }
    let fabric = db.load(&options).map_err(|e| e.to_string())?;
    if let Some(idcode) = device.idcode {
        fabric.check_idcode(idcode).map_err(|e| e.to_string())?;
    }
    let graph = fabric.arch.build_graph();
    let netlist = Netlist::build(design, top, device, &graph).map_err(|e| e.to_string())?;
    crate::flow::check_driverless(&netlist)?;
    Ok((Ecp5 { db, fabric }, Loaded { graph, netlist }))
}

impl Ecp5 {
    /// The router's options for this family: clocks steered onto the global network.
    pub fn route_options(&self, graph: &RoutingGraph) -> route::RouteOptions {
        route::RouteOptions {
            node_base: self
                .fabric
                .clock_node_costs(graph, trellis::CLOCK_PREFERENCE),
            ..route::RouteOptions::default()
        }
    }

    /// Every check `reticle fpga --bitstream` makes after routing, up to the bitstream it
    /// would write: the cells configured, block RAMs that do not overlap, every clock on a
    /// global network, no bit an arc needs clear set, and every set bit decoding back to
    /// exactly the arcs the router chose.
    #[allow(clippy::too_many_arguments)]
    pub fn check(
        &self,
        design: &Design,
        top: ModuleId,
        device: &Device,
        graph: &RoutingGraph,
        netlist: &Netlist,
        placement: &Placement,
        routing: &route::Routing,
    ) -> Result<Checked, String> {
        let fabric = &self.fabric;
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
        let pads = fabric
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
            return Err(format!("block RAMs overlap: {}", brams.overlaps.join("; ")));
        }
        let clocks = fabric.clock_network_use(netlist, placement, graph, routing);
        if !clocks.off_network.is_empty() {
            return Err(format!(
                "{} clock pin(s) off the global network: {}",
                clocks.off_network.len(),
                clocks.off_network.join(", ")
            ));
        }
        if pads == 0 {
            return Err("no pad was configured".to_owned());
        }
        let stream = fabric.stream(&tiles, "8").map_err(|e| e.to_string())?;
        let stolen = fabric.dropped_clear_bits(graph, routing, &tiles);
        if !stolen.is_empty() {
            return Err(format!(
                "{} bit(s) an arc needs clear are set: {}",
                stolen.len(),
                stolen.join("; ")
            ));
        }
        let decoded = self.db.decode(&stream.cram);
        if decoded.unexplained > 0 {
            let named: Vec<String> = decoded
                .leftovers
                .iter()
                .take(12)
                .map(|(ty, at, bit)| format!("{bit} of {ty} at (col {}, row {})", at.0, at.1))
                .collect();
            return Err(format!(
                "{} of {} set bit(s) decode to nothing: {}",
                decoded.unexplained,
                decoded.bits,
                named.join(", ")
            ));
        }
        let (selected, unresolved) = self.db.resolved_arcs(&decoded);
        let mut chosen = fabric.routed_arcs(graph, routing);
        chosen.extend(carries.sums.iter().cloned());
        if !unresolved.is_empty() || selected != chosen {
            return Err(format!(
                "the bits select {} arc(s) and the router chose {} ({} unresolved)",
                selected.len(),
                chosen.len(),
                unresolved.len()
            ));
        }
        Ok(Checked {
            bits: decoded.bits,
            arcs: chosen.len(),
        })
    }
}
