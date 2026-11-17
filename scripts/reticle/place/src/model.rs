//! A guide `PlacementProblem` from reticle's routing graph and mapped netlist.
//!
//! - **Sites** are the graph's sites, one guide site each, of the graph's kinds. A site is at
//!   its tile, scaled by [`SCALE`], and a slice's sites are a step apart within the tile, so
//!   that a chain that runs from slice to slice and on to the next tile moves by one step at
//!   every link (an ECP5 `CCU2C` chain: slices A, B, C, D, then A of the tile to the east).
//!   The cost works in whole tiles, as reticle's wirelength does.
//! - **Fixed items** are the cells a pin constraint puts on a package pin.
//! - **Macros** are the groups that dedicated wiring holds together, read off the graph as
//!   reticle reads them: a sink pin that a short walk back reaches from exactly one site of
//!   its driver's kind.
//! - **Shared resources** say what reticle's site rules say: pins of two sites that are one
//!   wire carry one signal; sites that exclude each other hold one cell; a pool of a tile's
//!   control wires carries as many signals as it has wires; a clock buffer goes only where
//!   its clock's pad can reach it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use guide::place::{Item, Layout, OCCUPANCY, Pin, PlacementProblem, Point, Rect, Site, SiteId};
use reticle::fpga::Constraints;
use reticle::fpga::arch::{Arch, NodeId, RoutingGraph};
use reticle::fpga::place::Netlist;

/// `(sink kind, sink role, driver kind, driver role)`, as reticle keys a relation.
type RelationKey = (String, String, String, String);

/// Members of a shared resource: sites and keys.
type Members = Vec<(SiteId, u32)>;

/// How many steps a tile is wide and tall, so that the slices of a tile have their own
/// columns: a chain from the last slice of a tile to the first of the next is one step.
pub const SCALE: i64 = 4;

/// How far a walk back from a sink pin goes looking for its driver's sites, as reticle's
/// `DEDICATED_WALK`.
const DEDICATED_WALK: usize = 8;

/// The most wires in a pool of a tile's own control wires, as reticle's `MAX_POOL`.
const MAX_POOL: usize = 8;

/// The most global clocks one cell reads.
const CLOCKS: usize = 4;

/// The most ways through one site.
const THROUGHS: usize = 4;

/// The guide problem, and how its items and sites are reticle's.
pub struct Model {
    /// The problem.
    pub problem: PlacementProblem,
    /// By guide item, the reticle instance.
    pub instance_of: Vec<usize>,
    /// What was built, in numbers, for the report.
    pub stats: BTreeMap<&'static str, usize>,
    /// By guide net, whether it is a global clock's, which the cost leaves out.
    pub global_net: Vec<bool>,
    /// Guide's columns and rows, and the tiles they are.
    pub grid: Grid,
}

/// A slice's index within its tile, from its bel's name, or `None`.
fn slice_of(bel: &str) -> Option<i64> {
    let rest = bel.strip_prefix("SLICE")?;
    // A 7-series slice: `SLICEL_X0_ALUT`, `SLICEM_X1_CARRY4`.
    if rest.contains("_X1_") {
        return Some(1);
    }
    if rest.contains("_X0_") {
        return Some(0);
    }
    let letter = rest.chars().next()?;
    ('A'..='H')
        .position(|c| c == letter)
        .map(|i| i as i64 % SCALE)
}

/// The columns and rows of tiles that have sites of the commonest kind, in order: guide's
/// coordinates count only these, so that a column or row of other things, as a 7-series clock
/// row, is no step for a carry chain that crosses it. A site in a column or row between two of
/// them is half a step past the one before.
pub struct Grid {
    /// By guide column, the tile column.
    pub xs: Vec<u32>,
    /// By guide row, the tile row.
    pub ys: Vec<u32>,
    /// By kept column, from before the first, the first tile column with sites between it and
    /// the next: what a site there is measured at.
    pub between_x: Vec<u32>,
    /// The same for rows.
    pub between_y: Vec<u32>,
    /// Where, past a kept column, the columns between it and the next are: past its last
    /// slice. A family whose tiles use every step for slices keeps every column.
    between_step_x: i64,
}

/// Where, past a kept row, the rows between it and the next are.
const BETWEEN_Y: i64 = SCALE / 2;

impl Grid {
    fn new(graph: &RoutingGraph) -> Grid {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for site in &graph.sites {
            *counts.entry(site.kind.as_str()).or_default() += 1;
        }
        let common = counts
            .iter()
            .max_by_key(|&(kind, n)| (*n, std::cmp::Reverse(*kind)))
            .map(|(kind, _)| *kind)
            .unwrap_or_default();
        let of = |axis: fn(&(u32, u32)) -> u32, all: bool| {
            let mut v: Vec<u32> = graph
                .sites
                .iter()
                .filter(|s| all || s.kind == common)
                .map(|s| axis(&s.tile))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let slices = graph
            .sites
            .iter()
            .filter_map(|s| slice_of(&s.bel))
            .max()
            .unwrap_or(0);
        let between_step_x = slices + 1;
        let xs = of(|t| t.0, between_step_x >= SCALE);
        let ys = of(|t| t.1, false);
        let between = |kept: &[u32], axis: fn(&(u32, u32)) -> u32| {
            let mut out: Vec<Option<u32>> = vec![None; kept.len() + 1];
            for site in &graph.sites {
                let t = axis(&site.tile);
                if let Err(i) = kept.binary_search(&t) {
                    out[i] = Some(out[i].map_or(t, |o: u32| o.min(t)));
                }
            }
            out.iter()
                .enumerate()
                .map(|(i, o)| o.unwrap_or_else(|| kept[i.min(kept.len() - 1)]))
                .collect()
        };
        Grid {
            between_x: between(&xs, |t| t.0),
            between_y: between(&ys, |t| t.1),
            xs,
            ys,
            between_step_x,
        }
    }

    /// A tile column or row in guide's steps: its rank among the kept ones, or
    /// [`BETWEEN`] past the kept one before it.
    fn step(kept: &[u32], t: u32, between: i64) -> i64 {
        match kept.binary_search(&t) {
            Ok(i) => i as i64 * SCALE,
            Err(i) => i as i64 * SCALE - SCALE + between,
        }
    }

    /// Where a site is, in guide's coordinates.
    pub fn point(&self, graph: &RoutingGraph, site: usize) -> Point {
        let s = &graph.sites[site];
        let (x, y) = s.tile;
        let cx = Grid::step(&self.xs, x, self.between_step_x);
        let sub = if self.xs.binary_search(&x).is_ok() {
            slice_of(&s.bel).unwrap_or(0)
        } else {
            0
        };
        Point::new(cx + sub, Grid::step(&self.ys, y, BETWEEN_Y))
    }

    /// The tile a site's guide point is in. Monotone, so that a bounding box's corners give
    /// its tiles' bounding box.
    pub fn tile(&self, at: Point) -> (i64, i64) {
        let axis = |v: i64, kept: &[u32], between: &[u32], step: i64| -> i64 {
            let i = v.div_euclid(SCALE);
            if v.rem_euclid(SCALE) < step {
                i64::from(kept[i.clamp(0, kept.len() as i64 - 1) as usize])
            } else {
                i64::from(between[(i + 1).clamp(0, between.len() as i64 - 1) as usize])
            }
        };
        (
            axis(at.x, &self.xs, &self.between_x, self.between_step_x),
            axis(at.y, &self.ys, &self.between_y, BETWEEN_Y),
        )
    }

    /// The guide rectangle of the sites in a rectangle of tiles.
    fn rect(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Rect {
        Rect::new(
            Point::new(
                Grid::step(&self.xs, x0, self.between_step_x),
                Grid::step(&self.ys, y0, BETWEEN_Y),
            ),
            Point::new(
                Grid::step(&self.xs, x1, self.between_step_x) + SCALE - 1,
                Grid::step(&self.ys, y1, BETWEEN_Y) + SCALE - 1,
            ),
        )
    }
}

/// The value a pin draws from a shared wire: its signal, or its constant.
fn pin_value(netlist: &Netlist, pin: usize) -> Option<u32> {
    let p = &netlist.pins[pin];
    match (p.signal, &p.constant) {
        (Some(signal), _) => Some(signal as u32),
        (None, Some(bit)) => Some(0x8000_0000 | (*bit as u32)),
        _ => None,
    }
}

/// Interns strings as small numbers.
#[derive(Default)]
struct Names(BTreeMap<String, u32>);

impl Names {
    fn id(&mut self, name: &str) -> u32 {
        let next = self.0.len() as u32;
        *self.0.entry(name.to_owned()).or_insert(next)
    }
}

/// Builds the problem.
pub fn build(
    netlist: &Netlist,
    arch: &Arch,
    graph: &RoutingGraph,
    constraints: &Constraints,
    options: &reticle::fpga::place::PlaceOptions,
    forbidden: &[String],
) -> Result<Model, String> {
    let mut stats: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut kinds = Names::default();
    let grid = Grid::new(graph);
    let sites: Vec<Site> = (0..graph.sites.len())
        .map(|s| Site::new(grid.point(graph, s), kinds.id(&graph.sites[s].kind)))
        .collect();
    let mut problem = PlacementProblem::new(Layout::Sites(sites));

    // Keys under which items draw: one per pin role.
    let mut roles = Names::default();
    // Regions from the constraints, by name.
    let mut region_ids = BTreeMap::new();
    for region in &constraints.regions {
        let rect = grid.rect(region.x0, region.y0, region.x1, region.y1);
        region_ids.insert(region.name.clone(), problem.add_region(rect));
    }
    if !constraints.rlocs.is_empty() {
        return Err(format!(
            "{} rloc constraint(s), which this harness does not place yet",
            constraints.rlocs.len()
        ));
    }
    // reticle keeps each group to the box its members were legalised into, grown by a
    // tile; guide's placement is not asked to.
    if !constraints.keep_hierarchy.is_empty() {
        stats.insert("keep_hierarchy_ignored", constraints.keep_hierarchy.len());
    }

    let mut fixed_site: Vec<Option<usize>> = Vec::new();
    for instance in &netlist.instances {
        fixed_site.push(match &instance.pin {
            Some(pin) => Some(
                arch.site_of_pin(pin)
                    .and_then(|name| graph.site_index(name))
                    .ok_or_else(|| {
                        format!("`{}` is on pin {pin}, which has no site", instance.name)
                    })?,
            ),
            None => None,
        });
    }
    let reach = reachability(netlist, graph, &fixed_site, &mut roles);

    // The kinds of the sites that ways through a lookup table go through.
    let through_kinds: BTreeSet<String> = options
        .route_throughs
        .nodes
        .iter()
        .map(|t| graph.sites[t.site].kind.clone())
        .collect();
    let mut instance_of = Vec::new();
    let mut sterilising: BTreeMap<(String, i32, i32), u32> = BTreeMap::new();
    for (index, instance) in netlist.instances.iter().enumerate() {
        let kind = kinds.0.get(&instance.kind).copied().ok_or_else(|| {
            format!(
                "no site of kind `{}` for `{}`",
                instance.kind, instance.name
            )
        })?;
        let mut item = Item::new().kind(kind);
        if let Some(site) = fixed_site[index] {
            item = item.fixed_at_site(SiteId(site as u32));
            *stats.entry("fixed").or_default() += 1;
        }
        for (instance, key, _) in &reach {
            if *instance == index {
                item = item.draw(*key, 0);
            }
        }
        for assignment in &constraints.region_assignments {
            if reticle::fpga::constraints::matches_glob(&assignment.pattern, &instance.name)
                && let Some(&region) = region_ids.get(&assignment.region)
            {
                item = item.region(region);
            }
        }
        for &pin in &instance.pins {
            if let Some(value) = pin_value(netlist, pin) {
                item = item.draw(roles.id(&netlist.pins[pin].role), value);
            }
        }
        // The global clocks a cell reads, each under a key of its own, for the clock regions.
        let mut clocks: Vec<u32> = instance
            .pins
            .iter()
            .filter(|&&p| !netlist.pins[p].output)
            .filter_map(|&p| netlist.pins[p].signal)
            .filter(|&s| {
                netlist.signals[s].driver.is_some_and(|d| {
                    let kind = netlist.instances[netlist.pins[d].instance].kind.as_str();
                    matches!(kind, "gb" | "bufh" | "bufr" | "bufio")
                })
            })
            .map(|s| s as u32)
            .collect();
        clocks.sort_unstable();
        clocks.dedup();
        if clocks.len() > CLOCKS {
            return Err(format!(
                "`{}` reads more than {CLOCKS} clocks",
                instance.name
            ));
        }
        for (k, clock) in clocks.iter().enumerate() {
            item = item.draw(roles.id(&format!("clock{k}")), *clock);
        }
        // A cell on a site that a way through a lookup table goes through shuts the way:
        // it draws a value of its own under each way's key.
        if through_kinds.contains(&instance.kind) {
            for k in 0..THROUGHS as u32 {
                item = item.draw(
                    roles.id(&format!("thru{k}")),
                    0x4000_0000 | (k << 27) | index as u32,
                );
            }
        }
        // A block RAM that makes another unusable draws under the offset, its own identity.
        for &(dx, dy) in netlist.sterilises.get(index).into_iter().flatten() {
            let key = roles.id(&format!("sterilise:{dx},{dy}"));
            item = item.draw(key, index as u32);
            sterilising.insert((instance.kind.clone(), dx, dy), key);
        }
        problem.add_item(item);
        instance_of.push(index);
    }
    // Each site of a sterilising kind, and the site of its kind the offset reaches: at most
    // one of the cell that sterilises and the cell there.
    for ((kind, dx, dy), key) in &sterilising {
        let at: BTreeMap<(u32, u32), Vec<usize>> = (0..graph.sites.len())
            .filter(|&s| graph.sites[s].kind == *kind)
            .fold(BTreeMap::new(), |mut map, s| {
                map.entry(graph.sites[s].tile)
                    .or_insert_with(Vec::new)
                    .push(s);
                map
            });
        for (&(x, y), here) in &at {
            let (Some(tx), Some(ty)) = (x.checked_add_signed(*dx), y.checked_add_signed(*dy))
            else {
                continue;
            };
            for &there in at.get(&(tx, ty)).into_iter().flatten() {
                for &s in here {
                    problem.add_shared(
                        1,
                        &[(SiteId(s as u32), *key), (SiteId(there as u32), OCCUPANCY)],
                    );
                    *stats.entry("sterilised pairs").or_default() += 1;
                }
            }
        }
    }

    // Nets: every signal with a driver and a sink, the driver first. A global buffer's
    // output is carried by the clock network, and costs nothing.
    let mut global_net = Vec::new();
    let mut signal_of_net = Vec::new();
    for (s, signal) in netlist.signals.iter().enumerate() {
        if !signal.is_routable() {
            continue;
        }
        let mut pins: Vec<Pin> = Vec::new();
        for &p in signal.driver.iter().chain(&signal.sinks) {
            let item = guide::place::ItemId(netlist.pins[p].instance as u32);
            if !pins.iter().any(|q| q.item == item) {
                pins.push(Pin::new(item));
            }
        }
        if pins.len() < 2 {
            continue;
        }
        let driver = &netlist.pins[signal.driver.unwrap_or_default()];
        // A global buffer's net is carried by the clock network, and costs nothing: it is
        // left out.
        if netlist.instances[driver.instance].kind == "gb" {
            *stats.entry("global nets").or_default() += 1;
            continue;
        }
        global_net.push(false);
        signal_of_net.push(s);
        problem.add_net(1, &pins);
    }
    stats.insert("nets", signal_of_net.len());

    let pins_at = pins_at(graph);
    add_macros(netlist, graph, &grid, &pins_at, &mut problem, &mut stats)?;
    {
        // Each way through a site, numbered among that site's ways.
        let mut through_at: BTreeMap<NodeId, Vec<(usize, usize)>> = BTreeMap::new();
        let mut per_site: BTreeMap<usize, usize> = BTreeMap::new();
        for t in &options.route_throughs.nodes {
            let k = per_site.entry(t.site).or_default();
            through_at.entry(t.node).or_default().push((t.site, *k));
            *k += 1;
        }
        if per_site.values().any(|&n| n > THROUGHS) {
            return Err("a site with more ways through it than this harness numbers".to_owned());
        }
        add_shared(
            graph,
            &through_at,
            &pins_at,
            &mut roles,
            &mut problem,
            &mut stats,
        );
    }
    // Each clock region carries so many clocks, as reticle's anneal charges for more.
    if !options.clock_regions.is_empty() && options.clock_region_lines > 0 {
        let width = arch.width;
        let mut regions: BTreeMap<u16, Members> = BTreeMap::new();
        for (s, site) in graph.sites.iter().enumerate() {
            let (x, y) = site.tile;
            let Some(&region) = options.clock_regions.get((y * width + x) as usize) else {
                continue;
            };
            if region == reticle::fpga::place::NO_CLOCK_REGION {
                continue;
            }
            for k in 0..CLOCKS {
                regions
                    .entry(region)
                    .or_default()
                    .push((SiteId(s as u32), roles.id(&format!("clock{k}"))));
            }
        }
        for members in regions.values() {
            problem.add_shared(options.clock_region_lines, members);
            *stats.entry("clock regions").or_default() += 1;
        }
    }
    // Sites the caller keeps empty.
    for name in forbidden {
        let site = graph
            .site_index(name)
            .ok_or_else(|| format!("no site named `{name}`"))?;
        problem.add_shared(0, &[(SiteId(site as u32), OCCUPANCY)]);
        *stats.entry("forbidden sites").or_default() += 1;
    }
    for (_, _, unreachable) in reach {
        *stats.entry("unreachable buffer sites").or_default() += unreachable.len();
        problem.add_shared(0, &unreachable);
    }

    Ok(Model {
        problem,
        instance_of,
        stats,
        global_net,
        grid,
    })
}

/// Which site pins are on each node.
fn pins_at(graph: &RoutingGraph) -> HashMap<NodeId, Vec<(usize, String)>> {
    let mut map: HashMap<NodeId, Vec<(usize, String)>> = HashMap::new();
    for (index, site) in graph.sites.iter().enumerate() {
        for (role, node) in &site.pins {
            map.entry(*node).or_default().push((index, role.clone()));
        }
    }
    map
}

/// The sites of the driver's kind a walk back from a sink pin reaches, or `None` if the walk
/// does not close within [`DEDICATED_WALK`] wires: reticle's `dedicated_drivers`.
fn dedicated_drivers(
    graph: &RoutingGraph,
    pins_at: &HashMap<NodeId, Vec<(usize, String)>>,
    site: usize,
    role: &str,
    driver_kind: &str,
    driver_role: &str,
) -> Option<Vec<usize>> {
    let mut seen: Vec<NodeId> = graph.sites[site].pin_nodes(role).collect();
    if seen.is_empty() {
        return Some(Vec::new());
    }
    let mut next = 0;
    while next < seen.len() {
        let node = seen[next];
        next += 1;
        for pip in graph.incoming(node) {
            let from = graph.pip(*pip).from;
            if !seen.contains(&from) {
                seen.push(from);
                if seen.len() > DEDICATED_WALK {
                    return None;
                }
            }
        }
    }
    let mut out = Vec::new();
    for node in &seen {
        for (other, other_role) in pins_at.get(node).into_iter().flatten() {
            if *other != site
                && other_role == driver_role
                && graph.sites[*other].kind == driver_kind
                && !out.contains(other)
            {
                out.push(*other);
            }
        }
    }
    Some(out)
}

/// For one `(sink kind, sink role, driver kind, driver role)`, the driver's site of every
/// sink site, if the connection is dedicated: reticle's `find_relation`, without its twins.
fn find_relation(
    graph: &RoutingGraph,
    pins_at: &HashMap<NodeId, Vec<(usize, String)>>,
    key: &RelationKey,
) -> Option<BTreeMap<usize, usize>> {
    let (sink_kind, sink_role, driver_kind, driver_role) = key;
    let mut driver_of = BTreeMap::new();
    let mut sinks_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (site, s) in graph.sites.iter().enumerate() {
        if s.kind != *sink_kind || s.pin(sink_role).is_none() {
            continue;
        }
        let drivers = dedicated_drivers(graph, pins_at, site, sink_role, driver_kind, driver_role)?;
        match drivers.as_slice() {
            [] => {}
            [one] => {
                if sinks_of.insert(*one, site).is_some() {
                    return None;
                }
                driver_of.insert(site, *one);
            }
            _ => return None,
        }
    }
    (!driver_of.is_empty()).then_some(driver_of)
}

/// Groups the instances that dedicated wiring ties together into macros, where every link of
/// a kind is the same offset in guide's coordinates.
fn add_macros(
    netlist: &Netlist,
    graph: &RoutingGraph,
    grid: &Grid,
    pins_at: &HashMap<NodeId, Vec<(usize, String)>>,
    problem: &mut PlacementProblem,
    stats: &mut BTreeMap<&'static str, usize>,
) -> Result<(), String> {
    let fixed = |i: usize| netlist.instances[i].pin.is_some();
    // A site's slot: how many sites of its kind at its point come before it, as guide counts.
    let mut seen: BTreeMap<(Point, &str), u32> = BTreeMap::new();
    let slot: Vec<u32> = (0..graph.sites.len())
        .map(|s| {
            let n = seen
                .entry((grid.point(graph, s), graph.sites[s].kind.as_str()))
                .or_default();
            *n += 1;
            *n - 1
        })
        .collect();
    let mut relations: BTreeMap<RelationKey, Option<(Point, u32, u32)>> = BTreeMap::new();
    // (driver instance, sink instance, sink's offset from the driver)
    let mut links: Vec<(usize, usize, Point)> = Vec::new();
    // By instance in a link, the slot it takes.
    let mut slots: BTreeMap<usize, u32> = BTreeMap::new();
    for signal in &netlist.signals {
        let Some(driver) = signal.driver else {
            continue;
        };
        let dpin = &netlist.pins[driver];
        let x = dpin.instance;
        if fixed(x) {
            continue;
        }
        for sink in &signal.sinks {
            let spin = &netlist.pins[*sink];
            let y = spin.instance;
            if y == x || fixed(y) {
                continue;
            }
            let key = (
                netlist.instances[y].kind.clone(),
                spin.role.clone(),
                netlist.instances[x].kind.clone(),
                dpin.role.clone(),
            );
            let offset = relations.entry(key.clone()).or_insert_with(|| {
                let relation = find_relation(graph, pins_at, &key)?;
                // The sink's offset from its driver, which has to be one for every pair.
                // And the slots of both, which have to be the same for every pair too.
                let mut offsets: BTreeSet<(Point, u32, u32)> = BTreeSet::new();
                for (&sink, &driver) in &relation {
                    offsets.insert((
                        grid.point(graph, sink).minus(grid.point(graph, driver)),
                        slot[driver],
                        slot[sink],
                    ));
                }
                if offsets.len() != 1 {
                    eprintln!(
                        "note: dedicated {key:?} has {} offsets {:?}; not a macro",
                        offsets.len(),
                        offsets.iter().take(4).collect::<Vec<_>>()
                    );
                    return None;
                }
                offsets.into_iter().next()
            });
            if let Some((offset, driver_slot, sink_slot)) = *offset {
                links.push((x, y, offset));
                slots.insert(x, driver_slot);
                slots.insert(y, sink_slot);
            }
        }
    }
    if links.is_empty() {
        return Ok(());
    }
    // Offsets of each instance from its group's first member.
    let mut position: BTreeMap<usize, (usize, Point)> = BTreeMap::new();
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &(x, y, _) in &links {
        for i in [x, y] {
            position.entry(i).or_insert_with(|| {
                groups.insert(i, vec![i]);
                (i, Point::default())
            });
        }
    }
    // Merge until nothing changes: a link puts y at x's position plus its offset.
    loop {
        let mut changed = false;
        for &(x, y, offset) in &links {
            let (gx, px) = position[&x];
            let (gy, py) = position[&y];
            if gx == gy {
                if py != px.offset(offset) {
                    return Err(format!(
                        "dedicated wiring puts `{}` in two places",
                        netlist.instances[y].name
                    ));
                }
                continue;
            }
            // Move y's group into x's: y's members shift by (px + offset - py).
            let shift = px.offset(offset).minus(py);
            let moved = groups.remove(&gy).unwrap_or_default();
            for m in &moved {
                let (_, p) = position[m];
                position.insert(*m, (gx, p.offset(shift)));
            }
            groups.entry(gx).or_default().extend(moved);
            changed = true;
        }
        if !changed {
            break;
        }
    }
    for members in groups.values() {
        let mut list: Vec<(guide::place::ItemId, Point, Option<u32>)> = members
            .iter()
            .map(|&m| {
                (
                    guide::place::ItemId(m as u32),
                    position[&m].1,
                    slots.get(&m).copied(),
                )
            })
            .collect();
        list.sort_by_key(|&(item, p, _)| (p.y, p.x, item));
        problem.add_macro_in_slots(&list);
        *stats.entry("macros").or_default() += 1;
        *stats.entry("macro members").or_default() += list.len();
    }
    Ok(())
}

/// The pool of a tile's own wires that feed a pin: reticle's `captive_pool`.
fn captive_pool(
    graph: &RoutingGraph,
    tile: (u32, u32),
    pin: NodeId,
    through: &BTreeMap<NodeId, Vec<(usize, usize)>>,
) -> Option<Vec<NodeId>> {
    let local = |node: NodeId| {
        let wire = graph.wire(node);
        !wire.global && wire.tile == tile
    };
    if !local(pin) {
        return None;
    }
    let mut set: Vec<NodeId> = vec![pin];
    let mut expanded: Vec<NodeId> = Vec::new();
    for _ in 0..MAX_POOL {
        let next = set.iter().position(|node| {
            let ways = graph.incoming(*node);
            !through.contains_key(node)
                && !ways.is_empty()
                && ways.iter().all(|pip| local(graph.pip(*pip).from))
        });
        let Some(at) = next else { return Some(set) };
        let node = set.remove(at);
        expanded.push(node);
        for pip in graph.incoming(node) {
            let from = graph.pip(*pip).from;
            if expanded.contains(&from) || set.len() >= MAX_POOL {
                return None;
            }
            if !set.contains(&from) {
                set.push(from);
            }
        }
    }
    None
}

/// The shared resources of every tile: pins on one wire, exclusions, and control pools.
fn add_shared(
    graph: &RoutingGraph,
    through_at: &BTreeMap<NodeId, Vec<(usize, usize)>>,
    pins_at: &HashMap<NodeId, Vec<(usize, String)>>,
    roles: &mut Names,
    problem: &mut PlacementProblem,
    stats: &mut BTreeMap<&'static str, usize>,
) {
    // Pins of two sites of one tile on one wire.
    let mut nodes: Vec<(&NodeId, &Vec<(usize, String)>)> = pins_at.iter().collect();
    nodes.sort_by_key(|(n, _)| **n);
    for (_, on) in nodes {
        let mut sites: Vec<usize> = on.iter().map(|(s, _)| *s).collect();
        sites.sort_unstable();
        sites.dedup();
        if sites.len() < 2 {
            continue;
        }
        let members: Vec<(SiteId, u32)> = on
            .iter()
            .map(|(s, role)| (SiteId(*s as u32), roles.id(role)))
            .collect();
        problem.add_shared(1, &members);
        *stats.entry("shared wires").or_default() += 1;
    }
    // Sites that exclude each other.
    for (s, site) in graph.sites.iter().enumerate() {
        for &other in &site.blocks {
            problem.add_shared(
                1,
                &[
                    (SiteId(s as u32), OCCUPANCY),
                    (SiteId(other as u32), OCCUPANCY),
                ],
            );
            *stats.entry("exclusions").or_default() += 1;
        }
    }
    // Pools of a tile's control wires that more than one site draws on.
    let mut by_tile: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (s, site) in graph.sites.iter().enumerate() {
        by_tile.entry(site.tile).or_default().push(s);
    }
    for (tile, sites) in by_tile {
        // (pool wires, members)
        let mut pools: Vec<(Vec<NodeId>, Members)> = Vec::new();
        for &s in &sites {
            for (role, node) in &graph.sites[s].pins {
                if graph.incoming(*node).is_empty() {
                    continue;
                }
                let Some(pool) = captive_pool(graph, tile, *node, through_at) else {
                    continue;
                };
                if pool.len() < 2 && pool == [*node] {
                    continue;
                }
                let member = (SiteId(s as u32), roles.id(role));
                // Merge with every pool it overlaps.
                let mut wires = pool;
                let mut members = vec![member];
                let mut k = 0;
                while k < pools.len() {
                    if pools[k].0.iter().any(|w| wires.contains(w)) {
                        let (w, m) = pools.swap_remove(k);
                        for x in w {
                            if !wires.contains(&x) {
                                wires.push(x);
                            }
                        }
                        members.extend(m);
                    } else {
                        k += 1;
                    }
                }
                pools.push((wires, members));
            }
        }
        for (wires, mut members) in pools {
            // A wire of the pool that is a bel's output carries what that bel drives and
            // nothing else: that output is a source of the pool, drawing its signal and
            // bringing its wire. A way through a lookup table is a wire any one signal may
            // take while the table's site is empty, and the cell on it shuts.
            let mut sources = Vec::new();
            let mut general = 0u32;
            for &wire in &wires {
                if !graph.incoming(wire).is_empty() {
                    general += 1;
                    for &(site, k) in through_at.get(&wire).into_iter().flatten() {
                        members.push((SiteId(site as u32), roles.id(&format!("thru{k}"))));
                    }
                    continue;
                }
                for (site, role) in pins_at.get(&wire).into_iter().flatten() {
                    let source = (SiteId(*site as u32), roles.id(role));
                    if graph.sites[*site].pin(role) == Some(wire) && !sources.contains(&source) {
                        sources.push(source);
                    }
                }
            }
            let mut drawing: Vec<SiteId> = members.iter().map(|(s, _)| *s).collect();
            drawing.sort_unstable();
            drawing.dedup();
            if drawing.len() < 2 || wires.len() > MAX_POOL {
                continue;
            }
            problem.add_shared_with_sources(general, &members, &sources);
            *stats.entry("pools").or_default() += 1;
        }
    }
}

/// For each global buffer driven by a fixed pad, a key it draws under and the buffer sites
/// that pad cannot reach, to be kept off: reticle's `confine_to_reachable`, for the case it
/// answers exactly.
fn reachability(
    netlist: &Netlist,
    graph: &RoutingGraph,
    fixed_site: &[Option<usize>],
    roles: &mut Names,
) -> Vec<(usize, u32, Members)> {
    let gb_sites: Vec<usize> = (0..graph.sites.len())
        .filter(|&s| graph.sites[s].kind == "gb")
        .collect();
    let mut out = Vec::new();
    for (index, instance) in netlist.instances.iter().enumerate() {
        if instance.kind != "gb" {
            continue;
        }
        let Some(input) = instance
            .pins
            .iter()
            .find(|&&p| !netlist.pins[p].output && netlist.pins[p].role == "i")
        else {
            continue;
        };
        let Some(signal) = netlist.pins[*input].signal else {
            continue;
        };
        let Some(driver) = netlist.signals[signal].driver else {
            continue;
        };
        let dpin = &netlist.pins[driver];
        let (Some(site), Some(pad)) = (
            fixed_site[dpin.instance],
            netlist.instances[dpin.instance].pin.as_ref(),
        ) else {
            continue;
        };
        let Some(start) = graph.sites[site].pin(&dpin.role) else {
            continue;
        };
        let targets: BTreeMap<NodeId, usize> = gb_sites
            .iter()
            .filter_map(|&s| Some((graph.sites[s].pin("i")?, s)))
            .collect();
        let mut seen = vec![false; graph.nodes.len()];
        let mut queue = std::collections::VecDeque::from([start]);
        seen[start as usize] = true;
        let mut reached = BTreeSet::new();
        while let Some(node) = queue.pop_front() {
            if let Some(&s) = targets.get(&node) {
                reached.insert(s);
                if reached.len() == targets.len() {
                    break;
                }
            }
            for pip in graph.outgoing(node) {
                let to = graph.pip(*pip).to;
                if !seen[to as usize] {
                    seen[to as usize] = true;
                    queue.push_back(to);
                }
            }
        }
        let key = roles.id(&format!("reach:{pad}"));
        let unreachable: Vec<(SiteId, u32)> = gb_sites
            .iter()
            .filter(|s| !reached.contains(s))
            .map(|&s| (SiteId(s as u32), key))
            .collect();
        if !unreachable.is_empty() {
            out.push((index, key, unreachable));
        }
    }
    out
}
