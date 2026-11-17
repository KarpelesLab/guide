//! What a device's sites and a design's netlist look like, for finding out what a placer
//! has to model.

use std::collections::BTreeMap;

use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::place::Netlist;

/// Prints the site kinds, one tile of each type, and the netlist's kinds and roles.
pub fn report(graph: &RoutingGraph, netlist: &Netlist) {
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for site in &graph.sites {
        *kinds.entry(site.kind.as_str()).or_default() += 1;
    }
    eprintln!(
        "graph: {} nodes, {} pips, {} sites",
        graph.nodes.len(),
        graph.pips.len(),
        graph.sites.len()
    );
    eprintln!("site kinds: {kinds:?}");
    let mut seen_types: BTreeMap<usize, (u32, u32)> = BTreeMap::new();
    for site in &graph.sites {
        seen_types.entry(site.tile_type).or_insert(site.tile);
    }
    for (tt, tile) in &seen_types {
        eprintln!("tile type {tt} at {tile:?}:");
        for (i, site) in graph.sites.iter().enumerate() {
            if site.tile == *tile && site.tile_type == *tt {
                let roles: Vec<String> = site
                    .pins
                    .iter()
                    .map(|(r, n)| format!("{r}={}", graph.nodes[*n as usize].name))
                    .collect();
                eprintln!(
                    "  [{i}] {} kind={} blocks={:?} pins={}",
                    site.bel,
                    site.kind,
                    site.blocks,
                    roles.join(" ")
                );
            }
        }
    }
    let mut ikinds: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for inst in &netlist.instances {
        *ikinds
            .entry((inst.kind.as_str(), inst.primitive.as_str()))
            .or_default() += 1;
    }
    eprintln!("instances: {ikinds:?}");
    let mut roles: BTreeMap<(&str, &str, bool), usize> = BTreeMap::new();
    for pin in &netlist.pins {
        let kind = netlist.instances[pin.instance].kind.as_str();
        *roles
            .entry((kind, pin.role.as_str(), pin.output))
            .or_default() += 1;
    }
    eprintln!("pin roles: {roles:?}");
    let routable = netlist.signals.iter().filter(|s| s.is_routable()).count();
    eprintln!(
        "signals: {} ({} routable), off fabric {}",
        netlist.signals.len(),
        routable,
        netlist.off_fabric.len()
    );
    let fixed: Vec<_> = netlist
        .instances
        .iter()
        .filter_map(|i| i.pin.as_ref().map(|p| format!("{}@{p}", i.name)))
        .collect();
    eprintln!("pinned: {fixed:?}");
}
