//! reticle's own iCE40 flow, `fpga::place_and_route`, on its built-in (synthetic) routing
//! architecture, with the placement step open.

use std::sync::Arc;

use reticle::fpga::arch::{Arch, RoutingGraph};
use reticle::fpga::place::{Netlist, Placement};
use reticle::fpga::{Device, bitstream, route};
use reticle::ir::{Design, ModuleId};

use crate::flow::{Checked, Loaded};

/// The architecture.
pub struct Ice40 {
    /// The architecture.
    pub arch: Arch,
}

/// The built-in architecture and graph for `device`, and the netlist on it.
pub fn load(design: &Design, top: ModuleId, device: &Device) -> Result<(Ice40, Loaded), String> {
    let arch = reticle::fpga::arch::architecture_for(&device.name)
        .ok_or_else(|| format!("no architecture for `{}`", device.name))?
        .clone();
    let graph: Arc<RoutingGraph> = reticle::fpga::arch::builtin_graph_for(&device.name)
        .ok_or_else(|| format!("no graph for `{}`", device.name))?;
    let graph = Arc::unwrap_or_clone(graph);
    let netlist = Netlist::build(design, top, device, &graph).map_err(|e| e.to_string())?;
    Ok((Ice40 { arch }, Loaded { graph, netlist }))
}

impl Ice40 {
    /// `place_and_route`'s router options: the defaults.
    pub fn route_options(&self) -> route::RouteOptions {
        route::RouteOptions::default()
    }

    /// `place_and_route`'s last step: the bitstream, generated from the placed and routed
    /// design, which fails if the architecture contradicts itself.
    pub fn check(
        &self,
        design: &Design,
        top: ModuleId,
        graph: &RoutingGraph,
        netlist: &Netlist,
        placement: &Placement,
        routing: &route::Routing,
    ) -> Result<Checked, String> {
        let tiles =
            bitstream::generate(design, top, &self.arch, graph, netlist, placement, routing)
                .map_err(|e| e.to_string())?;
        let bits = tiles.used_tiles().iter().map(|(_, b)| b.len()).sum();
        Ok(Checked { bits, arcs: 0 })
    }
}
