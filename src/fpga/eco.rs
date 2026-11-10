//! Engineering changes on an FPGA: carrying a placement and routes over to a netlist that has
//! changed, by the names of blocks and nets.
//!
//! The generic machinery is in [`crate::eco`] and [`crate::place::place_incremental`]. What is
//! particular here is only that a changed netlist numbers its blocks and nets afresh, so that
//! what they had before has to be found by name.

use std::collections::HashMap;

use super::design::Design;
use super::netlist::PackedNetlist;
use super::place::Placement;
use super::placer::PlaceProblem;
use crate::id::{NetId, NodeId};
use crate::net::{Route, RouteSet};
use crate::place::{ItemId, Prior, SiteId};

/// Where each block of `netlist` was in `placement`, found by name, as priors for
/// [`place_incremental`](crate::place::place_incremental) on `problem`, which was built from
/// `netlist`.
///
/// A block that `placement` does not name, or puts on no site of the block's type, has no
/// prior, and is placed as a new block.
pub fn placement_priors(
    problem: &PlaceProblem,
    netlist: &PackedNetlist,
    placement: &Placement,
) -> Vec<Option<Prior>> {
    let sites = problem.problem().sites();
    // Only looked up.
    let by_location: HashMap<(u32, u32, u32), SiteId> = (0..sites.len())
        .filter_map(|site| {
            let site = SiteId(site as u32);
            problem.site_location(site).map(|location| (location, site))
        })
        .collect();
    netlist
        .blocks()
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let placed = placement.block(&block.name)?;
            let site = *by_location.get(&(placed.x, placed.y, placed.sub_block))?;
            let kind = problem.problem().item(ItemId(index as u32)).site_kind();
            (sites[site.index()].kind == kind).then(|| Prior {
                origin: sites[site.index()].at,
                rotation: 0,
                site: Some(site),
            })
        })
        .collect()
}

/// The routes of `before`, a design that `routes` routed, for the nets of `after` with the same
/// names, by their IDs in `after`, to seed a run on `after` with: see
/// [`Engine::seed_routes`](crate::engine::Engine::seed_routes).
///
/// Nets are matched by name, so that this works when the netlist was renumbered. Routes of nets
/// that `after` does not route are left out.
pub fn route_seeds(
    before: &Design,
    routes: &RouteSet<NodeId>,
    after: &Design,
) -> Vec<(NetId, Route<NodeId>)> {
    // Only looked up.
    let ids: HashMap<&str, NetId> = after
        .nets()
        .iter()
        .filter_map(|net| Some((after.net_info(net.id)?.name.as_str(), net.id)))
        .collect();
    let mut seeds: Vec<(NetId, Route<NodeId>)> = routes
        .routes()
        .filter_map(|(net, route)| {
            let name = before.net_info(net)?.name.as_str();
            Some((*ids.get(name)?, route.clone()))
        })
        .collect();
    seeds.sort_unstable_by_key(|&(net, _)| net);
    seeds
}

#[cfg(test)]
mod tests {
    use super::super::tests::{line_netlist, mesh_graph};
    use super::super::{
        Architecture, NetOptions, PlacedBlock, ProblemOptions, build_place_problem, read_rr_graph,
        resolve_nets,
    };
    use super::*;
    use crate::eco::{reroute, stable_options};
    use crate::place::{AnalyticOptions, Wirelength, place_analytic};

    #[test]
    fn a_placement_and_its_routes_carry_over_by_name() {
        let device = read_rr_graph(mesh_graph(4, 2).as_bytes()).unwrap();
        let netlist = line_netlist(10);
        let problem = build_place_problem(
            &device,
            &netlist,
            &Architecture::default(),
            &ProblemOptions::default(),
        )
        .unwrap();
        let (placed, _) =
            place_analytic(problem.problem(), Wirelength, AnalyticOptions::default()).unwrap();
        let vpr = problem.to_placement(&netlist, &placed.placement, "t.net", "SHA256:0");
        assert_eq!(
            placement_priors(&problem, &netlist, &vpr),
            Prior::all(&placed.placement)
        );

        // A block the placement leaves out has no prior.
        let mut partial = Placement::new(None, None, vpr.array_size());
        for block in vpr.blocks().iter().filter(|block| block.name != "b3") {
            partial.add_block(PlacedBlock { ..block.clone() });
        }
        let priors = placement_priors(&problem, &netlist, &partial);
        let b3 = netlist
            .blocks()
            .iter()
            .position(|b| b.name == "b3")
            .unwrap();
        assert_eq!(priors.iter().filter(|prior| prior.is_none()).count(), 1);
        assert!(priors[b3].is_none());

        let design = resolve_nets(&device, &netlist, &vpr, &NetOptions::default()).unwrap();
        let options = device.engine_options();
        let before = crate::engine::route(device.graph(), design.nets(), options).unwrap();
        assert!(before.is_success());
        let seeds = route_seeds(&design, &before.routes, &design);
        assert_eq!(seeds.len(), before.routes.routes().count());
        let after = reroute(
            device.graph(),
            design.nets(),
            &before.routes,
            &[],
            stable_options(options),
        )
        .unwrap();
        assert!(after.outcome.is_success());
        assert_eq!(after.change.unchanged_fraction(), 1.0);
        assert_eq!(after.outcome.routes.stats.searches, 0);
    }
}
