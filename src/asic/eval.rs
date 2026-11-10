//! Scoring routes as the ISPD 2008 contest's `eval2008.pl` does.
//!
//! This shares nothing with the router but the reading of the design. It works from the
//! segments of a route file, in coordinates, as the script does, and follows the script's
//! rules, which are not all obvious:
//!
//! - **Usage.** Each wire takes, from every edge it crosses, its net's width or the layer's
//!   least width, whichever is more, plus the layer's spacing. A net that crosses an edge twice
//!   takes from it twice. Vias take nothing.
//! - **Overflow** of an edge is how far its usage exceeds its capacity, in the same units, so
//!   with a width and spacing of 1 one net too many is an overflow of 2. The total is the sum
//!   over all edges, and the maximum the largest.
//! - **Wirelength** is the number of edges between cells that wires cross, plus `via_cost`
//!   for each layer a via spans: 1 in 2008, and 3 in 2007.
//! - **Connections** are checked for nets of at most 1000 pins: everything a net's segments
//!   touch must be connected to its first pin, or the script stops with "disjoint", and a pin
//!   that isn't is reported as not attached. A net with no route whose pins are in more than one
//!   cell is unrouted. Nets of more than 1000 pins, and nets in one cell, need no route.
//! - A net routed twice in the file takes capacity for both, and counts the length of the last.

use super::format::{Capacities, Design, EdgeAt, Gcell, NetRoute, Point3};

/// Why the contest's script would stop on a route file, which it does at the first.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RouteFault {
    /// A route names a net that the design doesn't have.
    UnknownNet {
        /// The name.
        name: String,
    },
    /// A segment is neither a straight wire nor a via.
    Diagonal {
        /// The net's index in the design.
        net: usize,
    },
    /// A segment starts and ends in the same place.
    Null {
        /// The net's index in the design.
        net: usize,
    },
    /// A segment leaves the grid.
    OffGrid {
        /// The net's index in the design.
        net: usize,
    },
    /// A net's segments are not all connected to its first pin.
    Disjoint {
        /// The net's index in the design.
        net: usize,
    },
}

/// The scores of a set of routes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Evaluation {
    /// The sum of all edges' overflows.
    pub total_overflow: u64,
    /// The largest overflow of any edge.
    pub max_overflow: u64,
    /// How many edges have an overflow.
    pub overflowed_edges: u64,
    /// How many nets cross an edge with an overflow.
    pub overflowed_nets: u64,
    /// The length of wires, in edges crossed, plus the via cost times [`vias`](Self::vias).
    pub wirelength: u64,
    /// The length of wires alone, in edges crossed.
    pub wire_length: u64,
    /// How many layers vias span in all.
    pub vias: u64,
    /// Nets that need a route and have none, by index in the design.
    pub unrouted: Vec<usize>,
    /// Pins not connected to the first pin of their net.
    pub unattached_pins: u64,
    /// Nets whose segments form a cycle, which the script warns about.
    pub nets_with_cycles: u64,
    /// Everything that would have stopped the script, in the order found.
    pub faults: Vec<RouteFault>,
}

impl Evaluation {
    /// Whether the script would accept the routes: no faults, no unrouted net, and every pin
    /// attached.
    pub fn is_complete(&self) -> bool {
        self.faults.is_empty() && self.unrouted.is_empty() && self.unattached_pins == 0
    }
}

/// Usage of every edge, in capacity units.
struct Usage {
    capacities: Capacities,
    horizontal: Vec<u64>,
    vertical: Vec<u64>,
}

impl Usage {
    fn index(&self, at: EdgeAt) -> (bool, usize) {
        let (cols, rows) = (self.capacities.cols(), self.capacities.rows());
        match at {
            EdgeAt::Horizontal { layer, x, y } => (true, (layer * rows + y) * (cols - 1) + x),
            EdgeAt::Vertical { layer, x, y } => (false, (layer * (rows - 1) + y) * cols + x),
        }
    }

    fn add(&mut self, at: EdgeAt, amount: u64) {
        match self.index(at) {
            (true, i) => self.horizontal[i] += amount,
            (false, i) => self.vertical[i] += amount,
        }
    }

    fn overflow(&self, at: EdgeAt) -> u64 {
        let used = match self.index(at) {
            (true, i) => self.horizontal[i],
            (false, i) => self.vertical[i],
        };
        used.saturating_sub(u64::from(self.capacities.get(at)))
    }
}

/// One unit of a segment, between two cells next to each other on a layer or one above the
/// other.
#[derive(Clone, Copy)]
enum Unit {
    Edge(EdgeAt),
    Via,
}

/// A segment cut into the units it covers, with their two ends, or the fault in it.
fn units(
    design: &Design,
    net: usize,
    seg: &(Point3, Point3),
    out: &mut Vec<(Gcell, Gcell, Unit)>,
) -> Result<(), RouteFault> {
    let ((ax, ay, al), (bx, by, bl)) = *seg;
    let (Some((x1, y1)), Some((x2, y2))) = (design.cell_of(ax, ay), design.cell_of(bx, by)) else {
        return Err(RouteFault::OffGrid { net });
    };
    if usize::from(al.max(bl)) >= design.layers() {
        return Err(RouteFault::OffGrid { net });
    }
    if x1 != x2 {
        if y1 != y2 || al != bl {
            return Err(RouteFault::Diagonal { net });
        }
        for x in x1.min(x2)..x1.max(x2) {
            let edge = EdgeAt::Horizontal {
                layer: usize::from(al),
                x: x as usize,
                y: y1 as usize,
            };
            out.push((
                Gcell::new(x, y1, al),
                Gcell::new(x + 1, y1, al),
                Unit::Edge(edge),
            ));
        }
    } else if y1 != y2 {
        if al != bl {
            return Err(RouteFault::Diagonal { net });
        }
        for y in y1.min(y2)..y1.max(y2) {
            let edge = EdgeAt::Vertical {
                layer: usize::from(al),
                x: x1 as usize,
                y: y as usize,
            };
            out.push((
                Gcell::new(x1, y, al),
                Gcell::new(x1, y + 1, al),
                Unit::Edge(edge),
            ));
        }
    } else if al != bl {
        for l in al.min(bl)..al.max(bl) {
            out.push((Gcell::new(x1, y1, l), Gcell::new(x1, y1, l + 1), Unit::Via));
        }
    } else {
        return Err(RouteFault::Null { net });
    }
    Ok(())
}

/// Scores `routes` for `design` as `eval2008.pl` does, with `via_cost` for each layer a via
/// spans: 1 for the ISPD 2008 contest, 3 for the 2007 one.
///
/// A segment with a fault adds nothing, and the rest of its net is scored as if it weren't
/// there. The script stops at the first fault instead, so its numbers only mean something
/// when [`Evaluation::faults`] is empty.
pub fn evaluate(design: &Design, routes: &[NetRoute], via_cost: u64) -> Evaluation {
    let capacities = design.capacities();
    let (cols, rows, layers) = (capacities.cols(), capacities.rows(), capacities.layers());
    let mut usage = Usage {
        horizontal: vec![0; layers * cols.saturating_sub(1) * rows],
        vertical: vec![0; layers * cols * rows.saturating_sub(1)],
        capacities,
    };
    let mut by_name: Vec<(&str, usize)> = design
        .nets
        .iter()
        .enumerate()
        .map(|(i, n)| (n.name.as_str(), i))
        .collect();
    by_name.sort_unstable();
    let mut eval = Evaluation::default();
    // The units of each net's last route, which is what the script keeps for length.
    let mut last: Vec<Option<Vec<(Gcell, Gcell, Unit)>>> = vec![None; design.nets.len()];
    for route in routes {
        let Ok(at) = by_name.binary_search_by(|(n, _)| (*n).cmp(route.name.as_str())) else {
            eval.faults.push(RouteFault::UnknownNet {
                name: route.name.clone(),
            });
            continue;
        };
        let n = by_name[at].1;
        let net = &design.nets[n];
        let mut all = Vec::new();
        for seg in &route.segments {
            if let Err(fault) = units(design, n, seg, &mut all) {
                eval.faults.push(fault);
            }
        }
        for &(a, _, unit) in &all {
            if let Unit::Edge(edge) = unit {
                usage.add(
                    edge,
                    u64::from(design.wire_usage(net, usize::from(a.layer))),
                );
            }
        }
        if net.pins.len() <= 1000 {
            check_connections(design, n, &all, &mut eval);
        }
        last[n] = Some(all);
    }
    for (n, net) in design.nets.iter().enumerate() {
        match &last[n] {
            None => {
                if net.pins.len() <= 1000 && spans_cells(design, n) {
                    eval.unrouted.push(n);
                }
            }
            Some(units) => {
                let mut overflowed = false;
                for &(_, _, unit) in units {
                    match unit {
                        Unit::Edge(edge) => {
                            eval.wire_length += 1;
                            overflowed |= usage.overflow(edge) > 0;
                        }
                        Unit::Via => eval.vias += 1,
                    }
                }
                eval.overflowed_nets += u64::from(overflowed);
            }
        }
    }
    eval.wirelength = eval.wire_length + via_cost * eval.vias;
    for layer in 0..layers {
        for y in 0..rows {
            for x in 0..cols {
                let mut edges = [None, None];
                if x + 1 < cols {
                    edges[0] = Some(EdgeAt::Horizontal { layer, x, y });
                }
                if y + 1 < rows {
                    edges[1] = Some(EdgeAt::Vertical { layer, x, y });
                }
                for edge in edges.into_iter().flatten() {
                    let over = usage.overflow(edge);
                    if over > 0 {
                        eval.overflowed_edges += 1;
                        eval.total_overflow += over;
                        eval.max_overflow = eval.max_overflow.max(over);
                    }
                }
            }
        }
    }
    eval
}

/// Whether a net's pins are in more than one cell.
fn spans_cells(design: &Design, n: usize) -> bool {
    let mut cells = design.nets[n].pins.iter().map(|p| design.cell_of(p.x, p.y));
    let Some(first) = cells.next() else {
        return false;
    };
    cells.any(|c| c != first)
}

/// Checks that a net's units are all connected to its first pin, and its pins to them.
fn check_connections(
    design: &Design,
    n: usize,
    units: &[(Gcell, Gcell, Unit)],
    eval: &mut Evaluation,
) {
    let net = &design.nets[n];
    let Some(first) = net.pins.first() else {
        return;
    };
    let Some((fx, fy)) = design.cell_of(first.x, first.y) else {
        return;
    };
    // Every cell touched, numbered, and the connections between them both ways.
    let mut nodes: Vec<Gcell> = units.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    let start = Gcell::new(fx, fy, first.layer);
    nodes.push(start);
    nodes.sort_unstable();
    nodes.dedup();
    let index = |g: Gcell| nodes.binary_search(&g).ok();
    let mut links: Vec<(usize, usize)> = Vec::with_capacity(units.len() * 2);
    for &(a, b, _) in units {
        if let (Some(i), Some(j)) = (index(a), index(b)) {
            links.push((i, j));
            links.push((j, i));
        }
    }
    links.sort_unstable();
    let mut seen = vec![false; nodes.len()];
    let mut stack = Vec::new();
    if let Some(s) = index(start) {
        seen[s] = true;
        stack.push(s);
    }
    let mut reached_links = 0usize;
    let mut reached = 0usize;
    while let Some(i) = stack.pop() {
        reached += 1;
        let from = links.partition_point(|&(a, _)| a < i);
        for &(_, j) in links[from..].iter().take_while(|&&(a, _)| a == i) {
            reached_links += 1;
            if !seen[j] {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    // Each link is counted from both ends.
    if reached_links / 2 >= reached && reached > 0 {
        eval.nets_with_cycles += 1;
    }
    for pin in &net.pins {
        let attached = design
            .cell_of(pin.x, pin.y)
            .and_then(|(x, y)| index(Gcell::new(x, y, pin.layer)))
            .is_some_and(|i| seen[i]);
        if !attached {
            eval.unattached_pins += 1;
        }
    }
    if units
        .iter()
        .any(|&(a, b, _)| [a, b].iter().any(|&g| index(g).is_some_and(|i| !seen[i])))
    {
        eval.faults.push(RouteFault::Disjoint { net: n });
    }
}
