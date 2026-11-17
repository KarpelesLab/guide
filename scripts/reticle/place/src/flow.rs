//! What every family's flow shares.

use reticle::fpga::Constraints;
use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::place::Netlist;

/// A device's routing graph with a design's netlist on it.
pub struct Loaded {
    /// The routing graph, with its sites.
    pub graph: RoutingGraph,
    /// The mapped netlist.
    pub netlist: Netlist,
}

/// What the checks after routing found, when they passed.
pub struct Checked {
    /// Set bits of the configuration, every one decoded.
    pub bits: usize,
    /// Arcs the bits select, the same as the router chose.
    pub arcs: usize,
}

/// The one directory under `~/.cache/reticle/<name>/` that `reticle fetch` made.
pub fn cached(name: &str) -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_owned())?;
    let dir = std::path::Path::new(&home)
        .join(".cache/reticle")
        .join(name);
    let mut found: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("no {name} in {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();
    found.sort();
    found
        .pop()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| format!("no {name} in {}", dir.display()))
}

/// The one IO standard the constraints ask for, if any, as `reticle fpga` reads it.
pub fn single_io_standard(constraints: &Constraints) -> Result<Option<String>, String> {
    let mut standards: Vec<&str> = constraints
        .pins
        .iter()
        .filter_map(|p| p.io.io_standard.as_deref())
        .collect();
    standards.sort_unstable();
    standards.dedup();
    match standards.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some((*one).to_owned())),
        many => Err(format!("{} io standards: {}", many.len(), many.join(", "))),
    }
}

/// Refuses a netlist in which something reads a signal nothing drives, as `reticle fpga`
/// does.
pub fn check_driverless(netlist: &Netlist) -> Result<(), String> {
    let problems = netlist.driverless();
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "nothing drives {} signal(s): {}",
        problems.len(),
        problems
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    ))
}
