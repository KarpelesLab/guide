//! Placing a packed netlist on a device, with the placers of [`crate::place`].
//!
//! [`build_place_problem`] turns a packed netlist and a device into a
//! [`PlacementProblem`](crate::place::PlacementProblem) on typed sites, with carry chains as
//! macros and fixed blocks where a `.place` file puts them. [`place_design`] places it as
//! VPR's placer would weigh it, by wirelength and by timing, with
//! [`Annealer`](crate::place::Annealer) or [`place_analytic`](crate::place::place_analytic), and
//! [`PlaceProblem::to_placement`] gives a [`Placement`] that [`write_placement`] writes for VPR,
//! or that [`resolve_nets`](super::resolve_nets) takes to route.
//!
//! [`write_placement`]: super::write_placement

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::ImportError;
use super::arch::Architecture;
use super::lookahead::{Dijkstra, edge_delays};
use super::netlist::PackedNetlist;
use super::place::{PlacedBlock, Placement};
use super::rr_graph::{NodeKind, PinClassKind, RrGraph};
use super::timing::DesignTiming;
use crate::id::{NetId, NodeId};
use crate::place::{
    AnalyticOptions, AnnealOptions, Annealer, CostTerm, Item, ItemId, Layout, NetView, Pin,
    PlaceError, PlacementProblem, Point, Site, SiteId, Wirelength, legalize, place_analytic,
};
use crate::rng::Rng;
use crate::space::{RoutingSpace, Step};
use crate::timing::{ArcId, Constraints, CriticalityOptions, TimingAnalysis};

/// What [`build_place_problem`] needs besides the netlist and the device.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ProblemOptions<'a> {
    /// Blocks to fix where this placement puts them, as VPR's `--fix_clusters` does: I/O pads,
    /// say. Blocks it doesn't name are free. A block of a carry chain can't be fixed.
    pub fixed: Option<&'a Placement>,
    /// Nets that placement leaves out, besides those that reach a clock port, which are always
    /// left out as VPR leaves them to an ideal clock. VPR also leaves out constant nets, which
    /// [`constant_nets`](super::constant_nets) finds.
    pub ignored_nets: Vec<NetId>,
}

/// A packed netlist as a placement problem on a device's grid.
///
/// Each block of the netlist is an item, of the kind of the tile type of the same name, and
/// each position of each tile of a type that some block needs is a site, at the tile's lower
/// left corner in tiles. A pin is on the tile of the block where the graph has its `IPIN` or
/// `OPIN` nodes, so on a block over several tiles, as a memory, pins are on the tiles they
/// belong to, as VPR's `pin_width_offset` and `pin_height_offset` put them. Nets are the netlist's, the driver first, but for those that are
/// [left out](ProblemOptions::ignored_nets) or that have no driver or no sink.
///
/// Made by [`build_place_problem`].
#[derive(Clone, Debug)]
pub struct PlaceProblem {
    problem: PlacementProblem,
    grid: (u32, u32),
    /// Where each site is: column, row and position in the tile.
    sites: Vec<(u32, u32, u32)>,
    /// By problem net, the netlist's net.
    packed_net: Vec<u32>,
    /// By problem net, where its pins start among all nets' pins, and one more at the end.
    pin_start: Vec<u32>,
    /// By problem net, VPR's factor for nets of its size.
    crossing: Vec<f64>,
    /// How many carry chains became macros, and how many blocks they hold.
    chains: (usize, usize),
}

impl PlaceProblem {
    /// Returns the placement problem.
    pub fn problem(&self) -> &PlacementProblem {
        &self.problem
    }

    /// Returns the item of a block of the netlist, which has the block's number.
    pub fn item(&self, block: u32) -> ItemId {
        ItemId(block)
    }

    /// Returns where a site is, as the column, row and position in its tile that a `.place`
    /// file gives.
    pub fn site_location(&self, site: SiteId) -> Option<(u32, u32, u32)> {
        self.sites.get(site.index()).copied()
    }

    /// Returns the size of the device, in tiles, as width and height.
    pub fn grid_size(&self) -> (u32, u32) {
        self.grid
    }

    /// Returns the netlist's net that a net of the problem is.
    pub fn packed_net(&self, net: NetId) -> Option<NetId> {
        self.packed_net.get(net.index()).map(|&net| NetId(net))
    }

    /// Returns how many carry chains became macros, and how many blocks they hold in all.
    pub fn chains(&self) -> (usize, usize) {
        self.chains
    }

    /// Turns a placement of the problem into one VPR reads, with a header that names the
    /// netlist file and its digest, for each block in the netlist's order.
    ///
    /// # Panics
    ///
    /// If a block is on no site, which a legal placement never leaves it.
    pub fn to_placement(
        &self,
        netlist: &PackedNetlist,
        placement: &crate::place::Placement,
        netlist_file: &str,
        netlist_id: &str,
    ) -> Placement {
        let mut out = Placement::new(
            Some(netlist_file.to_owned()),
            Some(netlist_id.to_owned()),
            Some(self.grid),
        );
        for (index, block) in netlist.blocks().iter().enumerate() {
            let site = placement
                .site(ItemId(index as u32))
                .expect("a legal placement puts every block on a site");
            let (x, y, sub_block) = self.sites[site.index()];
            out.add_block(PlacedBlock {
                name: block.name.clone(),
                x,
                y,
                sub_block,
                layer: 0,
            });
        }
        out
    }

    /// Reads a placement that VPR wrote, or any other, into a placement of the problem, to
    /// measure it as [`place_design`] measures its own.
    ///
    /// # Errors
    ///
    /// [`ImportError::UnplacedBlock`] for a block the placement leaves out, and
    /// [`ImportError::BadLocation`] for one that it puts on no site of the block's type.
    pub fn from_placement(
        &self,
        netlist: &PackedNetlist,
        placement: &Placement,
    ) -> Result<crate::place::Placement, ImportError> {
        let by_location = self.site_index();
        let mut out = crate::place::Placement::new(&self.problem);
        for (index, block) in netlist.blocks().iter().enumerate() {
            let placed =
                placement
                    .block(&block.name)
                    .ok_or_else(|| ImportError::UnplacedBlock {
                        block: block.name.clone(),
                    })?;
            let item = ItemId(index as u32);
            let site = by_location
                .get(&(placed.x, placed.y, placed.sub_block))
                .copied()
                .filter(|&site| {
                    self.problem.sites()[site.index()].kind == self.problem.item(item).site_kind()
                })
                .ok_or_else(|| ImportError::BadLocation {
                    block: block.name.clone(),
                    x: placed.x,
                    y: placed.y,
                })?;
            out.set_site(&self.problem, item, site);
        }
        Ok(out)
    }

    /// Sites by column, row and position, only to look up.
    fn site_index(&self) -> HashMap<(u32, u32, u32), SiteId> {
        self.sites
            .iter()
            .enumerate()
            .map(|(site, &location)| (location, SiteId(site as u32)))
            .collect()
    }

    /// VPR's estimate of the wire a placement needs, which it prints as `BB estimate of
    /// min-dist (placement) wire length`: over nets, the width plus the height of the net's
    /// bounding box, counting tiles, times a factor for the net's size.
    ///
    /// The factor is VPR's `wirelength_crossing_count`, from 1 for nets of up to three pins
    /// to 2.79 for 50 and more beyond, after Cheng (1994). Pins are where
    /// [the problem](PlaceProblem) puts them. On a device whose channels are all as wide,
    /// this is also the bounding-box part of VPR's placement cost, which divides each
    /// direction by the average width of the channels the box spans.
    pub fn bb_estimate(&self, placement: &crate::place::Placement) -> f64 {
        let mut total = 0.0;
        for net in 0..self.problem.net_count() {
            total += self.net_bb(placement, NetId(net as u32));
        }
        total
    }

    /// One net's term of [`bb_estimate`](PlaceProblem::bb_estimate).
    fn net_bb(&self, placement: &crate::place::Placement, net: NetId) -> f64 {
        match self.problem.net_box(placement, net) {
            Some(bounds) => self.crossing[net.index()] * Self::span(bounds) as f64,
            None => 0.0,
        }
    }

    /// The width plus the height of a box, counting tiles, so a net within one tile spans 2.
    fn span(bounds: crate::place::Rect) -> u64 {
        bounds.width() + bounds.height() + 2
    }
}

/// VPR's `wirelength_crossing_count`: by how much a net of this many pins, on average, needs
/// more wire than the half-perimeter of its box, from `place.cpp`.
fn crossing_count(pins: usize) -> f64 {
    const CROSS_COUNT: [f64; 50] = [
        1.0, 1.0, 1.0, 1.0828, 1.1536, 1.2206, 1.2823, 1.3385, 1.3991, 1.4493, 1.4974, 1.5455,
        1.5937, 1.6418, 1.6899, 1.7304, 1.7709, 1.8114, 1.8519, 1.8924, 1.9288, 1.9652, 2.0015,
        2.0379, 2.0743, 2.1061, 2.1379, 2.1698, 2.2016, 2.2334, 2.2646, 2.2958, 2.3271, 2.3583,
        2.3895, 2.4187, 2.4479, 2.4772, 2.5064, 2.5356, 2.5610, 2.5864, 2.6117, 2.6371, 2.6625,
        2.6887, 2.7148, 2.7410, 2.7671, 2.7933,
    ];
    if pins > 50 {
        2.7933 + 0.02616 * (pins - 50) as f64
    } else {
        CROSS_COUNT[pins.max(1) - 1]
    }
}

/// Builds the placement problem of a packed netlist on a device.
///
/// # The device
///
/// The grid and its tile types come from the routing resource graph, which VPR writes for the
/// device it sized for the netlist, rather than from the architecture's layout, so the sites
/// are exactly those VPR has. A tile type holds as many blocks as its pins' names say it has
/// positions, [`BlockType::capacity`](super::BlockType::capacity).
///
/// # Carry chains
///
/// Each of the architecture's [direct connections](super::ArchDirect) whose offset stays
/// within a tile position (`z_offset` 0) links a block to the next wherever a net goes from the
/// first's `from_pin` port to the second's `to_pin`, as VPR's `alloc_and_load_placement_macros`
/// finds them. A chain of linked blocks becomes a macro, each block at the offset of the
/// direct connection from the one before, so that the connection can be routed. A chain with a
/// fixed block is left as single blocks.
///
/// # Errors
///
/// [`ImportError::NoTileType`] if a block's type is not a tile type of the device, and
/// [`ImportError::BadLocation`] if a fixed block is not on a site of its type.
///
/// # Limits
///
/// A block goes on tiles of the type of its own name, which is how VTR's architectures have
/// it. `equivalent_sites` that let a block go on tiles of other types are not followed. Only
/// the first layer of a 3D device is used.
pub fn build_place_problem(
    device: &RrGraph,
    netlist: &PackedNetlist,
    arch: &Architecture,
    options: &ProblemOptions<'_>,
) -> Result<PlaceProblem, ImportError> {
    let types = device.block_types();
    let by_name: BTreeMap<&str, usize> = types
        .iter()
        .enumerate()
        .map(|(index, block_type)| (block_type.name.as_str(), index))
        .collect();
    let mut kind_of = Vec::with_capacity(netlist.blocks().len());
    for block in netlist.blocks() {
        let kind = by_name
            .get(block.pb_type.as_str())
            .copied()
            .ok_or_else(|| ImportError::NoTileType {
                block: block.name.clone(),
                pb_type: block.pb_type.clone(),
            })?;
        kind_of.push(kind);
    }
    let used: BTreeSet<usize> = kind_of.iter().copied().collect();

    // Sites, row by row, and the first tile of each type, to find pins on.
    let (width, height) = device.grid_size();
    let mut sites = Vec::new();
    let mut locations = Vec::new();
    let mut first_tile: BTreeMap<usize, (u32, u32)> = BTreeMap::new();
    for y in 0..height {
        for x in 0..width {
            let Some(tile) = device.tile(x, y, 0) else {
                continue;
            };
            if tile.width_offset != 0 || tile.height_offset != 0 {
                continue;
            }
            let kind = usize::from(tile.block_type);
            if !used.contains(&kind) {
                continue;
            }
            first_tile.entry(kind).or_insert((x, y));
            for sub_block in 0..types[kind].capacity() {
                sites.push(Site::new(
                    Point::new(i64::from(x), i64::from(y)),
                    kind as u32,
                ));
                locations.push((x, y, sub_block));
            }
        }
    }

    // Where each pin is on its tile, from the first tile of each type: as VPR's
    // `pin_width_offset` and `pin_height_offset` have it, the average, rounded down, of where
    // the pin's nodes are, one for each side of each tile it is on.
    let mut pin_offsets: BTreeMap<(usize, u32), Point> = BTreeMap::new();
    for (&kind, &(x, y)) in &first_tile {
        let block_type = &types[kind];
        for pins in &block_type.classes {
            let node_kind = match pins.kind {
                PinClassKind::Output => NodeKind::Opin,
                PinClassKind::Input => NodeKind::Ipin,
                _ => continue,
            };
            for pin in &pins.pins {
                let (mut sum, mut count) = (Point::default(), 0i64);
                for dy in 0..block_type.height.max(1) {
                    for dx in 0..block_type.width.max(1) {
                        for node in device.find_nodes(node_kind, x + dx, y + dy, pin.ptc) {
                            // A node found from a tile it only passes over is counted at its
                            // own.
                            if let Some(at) = device.node_location(node)
                                && (u32::from(at.xlow), u32::from(at.ylow)) == (x + dx, y + dy)
                            {
                                sum = sum.offset(Point::new(i64::from(dx), i64::from(dy)));
                                count += 1;
                            }
                        }
                    }
                }
                if count > 0 {
                    pin_offsets.insert((kind, pin.ptc), Point::new(sum.x / count, sum.y / count));
                }
            }
        }
    }
    let pin_offset = |pin: super::PackedPin| -> Point {
        let kind = kind_of[pin.block as usize];
        let port = netlist
            .ports()
            .get(pin.port as usize)
            .map_or("", String::as_str);
        types[kind]
            .find_pin_ptc(0, port, pin.bit)
            .and_then(|ptc| pin_offsets.get(&(kind, ptc)))
            .copied()
            .unwrap_or_default()
    };

    let mut problem = PlacementProblem::new(Layout::Sites(sites));

    // Fixed blocks.
    let site_index: HashMap<(u32, u32, u32), u32> = locations
        .iter()
        .enumerate()
        .map(|(site, &location)| (location, site as u32))
        .collect();
    let mut fixed_at: Vec<Option<SiteId>> = vec![None; netlist.blocks().len()];
    if let Some(fixed) = options.fixed {
        for (index, block) in netlist.blocks().iter().enumerate() {
            let Some(placed) = fixed.block(&block.name) else {
                continue;
            };
            let site = site_index
                .get(&(placed.x, placed.y, placed.sub_block))
                .copied()
                .filter(|&site| problem.sites()[site as usize].kind == kind_of[index] as u32)
                .ok_or_else(|| ImportError::BadLocation {
                    block: block.name.clone(),
                    x: placed.x,
                    y: placed.y,
                })?;
            fixed_at[index] = Some(SiteId(site));
        }
    }
    for (index, &kind) in kind_of.iter().enumerate() {
        let item = Item::new().kind(kind as u32);
        problem.add_item(match fixed_at[index] {
            Some(site) => item.fixed_at_site(site),
            None => item,
        });
    }

    // Nets.
    let mut ignored = options.ignored_nets.clone();
    ignored.sort_unstable();
    let mut packed_net = Vec::new();
    let mut pin_start = vec![0u32];
    let mut crossing = Vec::new();
    let mut pins = Vec::new();
    for (index, net) in netlist.nets().iter().enumerate() {
        let Some(driver) = net.driver else {
            continue;
        };
        if net.on_clock_port
            || net.sinks.is_empty()
            || ignored.binary_search(&NetId(index as u32)).is_ok()
        {
            continue;
        }
        pins.clear();
        for &pin in core::iter::once(&driver).chain(&net.sinks) {
            let offset = pin_offset(pin);
            pins.push(Pin::at(ItemId(pin.block), offset.x, offset.y));
        }
        problem.add_net(1, &pins);
        packed_net.push(index as u32);
        pin_start.push(pin_start[pin_start.len() - 1] + pins.len() as u32);
        crossing.push(crossing_count(pins.len()));
    }

    // Carry chains.
    let mut next: BTreeMap<u32, (u32, Point)> = BTreeMap::new();
    let mut has_previous = vec![false; netlist.blocks().len()];
    let blocks = netlist.blocks();
    let port_name = |port: u32| {
        netlist
            .ports()
            .get(port as usize)
            .map_or("", String::as_str)
    };
    for direct in arch.directs() {
        if direct.z_offset != 0 {
            continue;
        }
        let offset = Point::new(i64::from(direct.x_offset), i64::from(direct.y_offset));
        for net in netlist.nets() {
            let Some(driver) = net.driver else {
                continue;
            };
            if blocks[driver.block as usize].pb_type != direct.from_type
                || port_name(driver.port) != direct.from_port
            {
                continue;
            }
            let Some(sink) = net.sinks.iter().find(|sink| {
                sink.block != driver.block
                    && blocks[sink.block as usize].pb_type == direct.to_type
                    && port_name(sink.port) == direct.to_port
            }) else {
                continue;
            };
            if next.contains_key(&driver.block) || has_previous[sink.block as usize] {
                continue;
            }
            next.insert(driver.block, (sink.block, offset));
            has_previous[sink.block as usize] = true;
        }
    }
    let mut chains = (0, 0);
    let mut in_chain = vec![false; netlist.blocks().len()];
    for &head in next.keys() {
        if has_previous[head as usize] {
            continue;
        }
        let mut members = vec![(ItemId(head), Point::default())];
        in_chain[head as usize] = true;
        let mut at = Point::default();
        let mut block = head;
        while let Some(&(following, offset)) = next.get(&block) {
            if in_chain[following as usize] {
                break;
            }
            in_chain[following as usize] = true;
            at = at.offset(offset);
            members.push((ItemId(following), at));
            block = following;
        }
        if members.len() < 2
            || members
                .iter()
                .any(|&(item, _)| fixed_at[item.index()].is_some())
        {
            continue;
        }
        chains.0 += 1;
        chains.1 += members.len();
        problem.add_macro(&members);
    }

    Ok(PlaceProblem {
        problem,
        grid: (width, height),
        sites: locations,
        packed_net,
        pin_start,
        crossing,
        chains,
    })
}

/// The least delay of a connection between blocks some tiles apart, as VPR's `delta` delay
/// model for placement has it.
///
/// VPR routes, for each offset, from a block of the device's most common type to a block that
/// far away, and keeps the least delay over a few starting places. This searches the graph
/// for the least delay from every `SOURCE` of a tile of the most common type, to every `SINK`,
/// from five places: near the four corners of where that type is, and its middle. Each entry
/// is the least over the places and the `SINK`s at that offset, by column and row apart, which
/// is VPR's default reducer, `min`. The search leaves out edges from an `OPIN` straight to an
/// `IPIN`, the direct connections of carry chains, which only reach the next block of a chain,
/// as VPR's model, from the pins that reach the channels, leaves them out too. An offset that no
/// search reached takes the larger of its neighbours nearer the origin.
///
/// Delays are in picoseconds, the graph's own: see [`RrGraph::step_delay`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceDelayModel {
    width: u32,
    height: u32,
    delays: Vec<u32>,
}

impl PlaceDelayModel {
    /// Builds the model from a device's graph.
    pub fn from_rr_graph(device: &RrGraph) -> PlaceDelayModel {
        let (width, height) = device.grid_size();
        let (width, height) = (width.max(1), height.max(1));
        let types = device.block_types();
        // The most common type that holds something, and where it is.
        let mut roots: BTreeMap<u16, Vec<(u32, u32)>> = BTreeMap::new();
        for y in 0..height {
            for x in 0..width {
                if let Some(tile) = device.tile(x, y, 0)
                    && tile.width_offset == 0
                    && tile.height_offset == 0
                    && types
                        .get(usize::from(tile.block_type))
                        .is_some_and(|t| t.capacity() > 0)
                {
                    roots.entry(tile.block_type).or_default().push((x, y));
                }
            }
        }
        let common = roots
            .values()
            .fold(None::<&Vec<(u32, u32)>>, |best, tiles| match best {
                Some(best) if best.len() >= tiles.len() => Some(best),
                _ => Some(tiles),
            })
            .cloned()
            .unwrap_or_default();

        let space = device.graph();
        let count = space.num_nodes() as usize;
        let mut sinks = Vec::new();
        let mut is_ipin = vec![false; count];
        let mut is_opin = vec![false; count];
        let mut sources_at: BTreeMap<(u16, u16), Vec<u32>> = BTreeMap::new();
        for node in 0..count {
            let id = NodeId(node as u32);
            let Some(at) = device.node_location(id) else {
                continue;
            };
            match device.node_kind(id) {
                Some(NodeKind::Sink) => sinks.push((node, at.xlow, at.ylow)),
                Some(NodeKind::Source) => sources_at
                    .entry((at.xlow, at.ylow))
                    .or_default()
                    .push(node as u32),
                Some(NodeKind::Ipin) => is_ipin[node] = true,
                Some(NodeKind::Opin) => is_opin[node] = true,
                _ => {}
            }
        }

        let cells = (width * height) as usize;
        let mut delays = vec![u32::MAX; cells];
        if !common.is_empty() {
            let edge_delays = edge_delays(device);
            let mut dijkstra = Dijkstra::new(count);
            let mut steps: Vec<Step<NodeId>> = Vec::new();
            for (sx, sy) in pick_corners(&common) {
                let Some(sources) = sources_at.get(&(sx as u16, sy as u16)) else {
                    continue;
                };
                dijkstra.run(sources, |from, out| {
                    let range = device.edge_range(NodeId(from));
                    steps.clear();
                    space.expand(NetId(0), NodeId(from), &mut steps);
                    let direct = is_opin[from as usize];
                    out.extend(
                        steps
                            .iter()
                            .zip(&edge_delays[range])
                            .filter(|(step, _)| !(direct && is_ipin[step.to.index()]))
                            .map(|(step, &delay)| (step.to.0, f64::from(delay))),
                    );
                });
                for &(node, x, y) in &sinks {
                    if let Some(delay) = dijkstra.reached(node) {
                        let dx = u32::from(x).abs_diff(sx);
                        let dy = u32::from(y).abs_diff(sy);
                        let cell = (dy * width + dx) as usize;
                        // Sums of whole picoseconds, which `f64` holds exactly.
                        delays[cell] = delays[cell].min(delay as u32);
                    }
                }
            }
        }
        // Holes take the larger of the entries nearer the origin.
        for dy in 0..height {
            for dx in 0..width {
                let cell = (dy * width + dx) as usize;
                if delays[cell] != u32::MAX {
                    continue;
                }
                let left = (dx > 0).then(|| delays[cell - 1]);
                let below = (dy > 0).then(|| delays[cell - width as usize]);
                delays[cell] = match (left, below) {
                    (Some(a), Some(b)) => a.max(b),
                    (Some(a), None) | (None, Some(a)) => a,
                    (None, None) => 0,
                };
            }
        }
        PlaceDelayModel {
            width,
            height,
            delays,
        }
    }

    /// The delay of a connection between blocks `dx` columns and `dy` rows apart, in
    /// picoseconds. Offsets beyond the device take its far corner's.
    pub fn delay(&self, dx: u64, dy: u64) -> u32 {
        let dx = dx.min(u64::from(self.width) - 1) as usize;
        let dy = dy.min(u64::from(self.height) - 1) as usize;
        self.delays[dy * self.width as usize + dx]
    }

    /// The delay between two points in tiles.
    fn between(&self, a: Point, b: Point) -> u32 {
        self.delay(a.x.abs_diff(b.x), a.y.abs_diff(b.y))
    }
}

/// The tiles nearest the four corners and the middle of the box of `tiles`, each once, the
/// first nearest in order on a tie.
fn pick_corners(tiles: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let (mut low, mut high) = (tiles[0], tiles[0]);
    for &(x, y) in tiles {
        low = (low.0.min(x), low.1.min(y));
        high = (high.0.max(x), high.1.max(y));
    }
    let middle = ((low.0 + high.0) / 2, (low.1 + high.1) / 2);
    let mut picked = Vec::new();
    for point in [low, (high.0, low.1), (low.0, high.1), high, middle] {
        let nearest = tiles
            .iter()
            .copied()
            .min_by_key(|&(x, y)| x.abs_diff(point.0) + y.abs_diff(point.1));
        if let Some(nearest) = nearest
            && !picked.contains(&nearest)
        {
            picked.push(nearest);
        }
    }
    picked
}

/// How [`place_design`] goes about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlaceMethod {
    /// Annealing from a random placement, hot, as VPR did before version 9.
    Anneal,
    /// [`place_analytic`]'s global placement and legalization, by wirelength, and then
    /// annealing that starts cold over short distances, as VPR 9 starts cold from a placement
    /// that puts each block near the centroid of its neighbours. The default.
    Analytic,
}

/// How [`place_design`] places.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PlacerOptions {
    /// The seed of every random choice. The default is 1.
    pub seed: u64,
    /// Annealing after analytic placement, the default, or from scratch.
    pub method: PlaceMethod,
    /// Moves per temperature, as a multiple of `blocks^(4/3)`: VPR's `--inner_num`. The
    /// default is 1, where VPR's is 0.5: on VTR's benchmarks, see `docs/benchmarks.md`, 1
    /// came to about two thirds of VPR's time, and 0.5 to two fifths, for 0.6% more wire.
    pub inner_num: f64,
    /// How much timing counts against wirelength, from 0 for wirelength alone to 1 for timing
    /// alone: VPR's `--timing_tradeoff`, whose default is 0.5, as this one's is. Without a
    /// timing graph, it is 0.
    pub timing_tradeoff: f64,
    /// The exponent of criticalities at the start and at the end of annealing, VPR's
    /// `--td_place_exp_first` and `--td_place_exp_last`, 1 and 8 by default. In between it
    /// grows as the range limit shrinks, rounded to a whole number.
    pub criticality_exponents: (u32, u32),
    /// For [`PlaceMethod::Analytic`], the first temperature of annealing, as a multiple of the
    /// standard deviation of moves' cost changes. The default is 0.05, as
    /// [`AnnealOptions::detailed`] has it.
    pub detailed_temperature: f64,
    /// For [`PlaceMethod::Analytic`], the first range limit of annealing, in steps of the
    /// columns and rows that have sites of a block's type. The default is 8.
    pub detailed_range: u32,
}

impl Default for PlacerOptions {
    fn default() -> Self {
        PlacerOptions {
            seed: 1,
            method: PlaceMethod::Analytic,
            inner_num: 1.0,
            timing_tradeoff: 0.5,
            criticality_exponents: (1, 8),
            detailed_temperature: 0.05,
            detailed_range: 8,
        }
    }
}

/// What [`place_design`] did.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PlaceResult {
    /// The placement, which is legal.
    pub placement: crate::place::Placement,
    /// Its [`bb_estimate`](PlaceProblem::bb_estimate).
    pub bb_estimate: f64,
    /// That of the placement annealing started from: random, or legalized after global
    /// placement.
    pub start_bb_estimate: f64,
    /// The worst slack with connections' delays from the [`PlaceDelayModel`], in picoseconds,
    /// if there was a timing graph. Against VPR's default constraints, a period of 0, this is
    /// the critical path's delay, negated, which is what VPR prints as its placement's
    /// `estimated critical path delay`.
    pub worst_slack: Option<i64>,
    /// Temperatures annealing went through.
    pub temperatures: u32,
    /// Moves annealing tried.
    pub moves: u64,
}

/// The total of a placement's cost, made to stay near this, so that the temperature, which is
/// in the same units, means the same from one temperature to the next. Large enough that a
/// net's cost, rounded to a whole number, loses nothing that matters.
const COST_SCALE: f64 = 1e9;

/// VPR's placement cost of a net: its bounding box, as [`PlaceProblem::bb_estimate`] has it,
/// and the delays of its connections from the delay model, each times its criticality, each
/// weighed so that the two totals are `1 - tradeoff` and `tradeoff` of [`COST_SCALE`] for the
/// placement the weights were made for.
struct VprCost<'p> {
    problem: &'p PlaceProblem,
    delays: &'p PlaceDelayModel,
    bb_weight: f64,
    timing_weight: f64,
    /// By pin, among all nets' pins, the criticality of its connection, or nothing.
    criticality: Vec<f64>,
}

impl CostTerm for VprCost<'_> {
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        let index = net.net.index();
        let mut cost =
            self.bb_weight * self.problem.crossing[index] * PlaceProblem::span(net.bounds) as f64;
        if self.timing_weight > 0.0 {
            let start = self.problem.pin_start[index] as usize;
            if let Some((&driver, sinks)) = net.pins.split_first() {
                let mut timing = 0.0;
                for (k, &sink) in sinks.iter().enumerate() {
                    let criticality = self.criticality[start + 1 + k];
                    timing += criticality * f64::from(self.delays.between(driver, sink));
                }
                cost += self.timing_weight * timing;
            }
        }
        cost.round() as u64
    }
}

/// What reweighs the cost from one temperature to the next: a timing graph and an analysis
/// of it, and the arc of every connection.
struct Weigher<'p, 't> {
    problem: &'p PlaceProblem,
    delays: &'p PlaceDelayModel,
    timing: Option<TimingState<'t>>,
    tradeoff: f64,
}

struct TimingState<'t> {
    timing: &'t mut DesignTiming,
    constraints: &'t Constraints,
    analysis: TimingAnalysis,
    /// By pin, among all nets' pins, the arc of its connection, or nothing for a driver.
    arcs: Vec<Option<ArcId>>,
}

impl<'p, 't> Weigher<'p, 't> {
    fn new(
        problem: &'p PlaceProblem,
        delays: &'p PlaceDelayModel,
        timing: Option<(&'t mut DesignTiming, &'t Constraints)>,
        tradeoff: f64,
    ) -> Self {
        let timing = timing.map(|(timing, constraints)| {
            let mut arcs =
                Vec::with_capacity(problem.pin_start.last().copied().unwrap_or(0) as usize);
            for (net, &packed) in problem.packed_net.iter().enumerate() {
                let pins = problem.pin_start[net + 1] - problem.pin_start[net];
                arcs.push(None);
                for k in 1..pins {
                    arcs.push(timing.net_arc(NetId(packed), k));
                }
            }
            TimingState {
                analysis: TimingAnalysis::new(timing.graph()),
                timing,
                constraints,
                arcs,
            }
        });
        Weigher {
            problem,
            delays,
            timing,
            tradeoff: if tradeoff > 0.0 {
                tradeoff.min(1.0)
            } else {
                0.0
            },
        }
    }

    /// Times a placement, with connections' delays from the model, and returns the worst
    /// slack.
    fn analyse(&mut self, placement: &crate::place::Placement) -> Option<i64> {
        let state = self.timing.as_mut()?;
        let problem = &self.problem.problem;
        for net in 0..problem.net_count() {
            let pins = problem.net_pins(NetId(net as u32));
            let start = self.problem.pin_start[net] as usize;
            let driver = problem.pin_position(placement, pins[0]);
            for (k, &pin) in pins.iter().enumerate().skip(1) {
                if let Some(arc) = state.arcs[start + k] {
                    let delay = self
                        .delays
                        .between(driver, problem.pin_position(placement, pin));
                    let _ = state.timing.graph_mut().set_delay(arc, delay);
                }
            }
        }
        state.analysis.run(state.timing.graph(), state.constraints);
        state.analysis.worst_slack()
    }

    /// The cost for a placement, weighed for it, with criticalities to this exponent.
    fn cost(&mut self, placement: &crate::place::Placement, exponent: u32) -> VprCost<'p> {
        let problem = self.problem;
        let bb_total = problem.bb_estimate(placement).max(f64::MIN_POSITIVE);
        let mut cost = VprCost {
            problem,
            delays: self.delays,
            bb_weight: COST_SCALE / bb_total,
            timing_weight: 0.0,
            criticality: Vec::new(),
        };
        if self.tradeoff <= 0.0 || self.timing.is_none() {
            return cost;
        }
        self.analyse(placement);
        let Some(state) = self.timing.as_ref() else {
            return cost;
        };
        let options = CriticalityOptions::new(exponent, 1.0).unwrap_or_default();
        let places = &problem.problem;
        let mut criticality = vec![0.0; state.arcs.len()];
        let mut timing_total = 0.0;
        for net in 0..places.net_count() {
            let pins = places.net_pins(NetId(net as u32));
            let start = problem.pin_start[net] as usize;
            let driver = places.pin_position(placement, pins[0]);
            for (k, &pin) in pins.iter().enumerate().skip(1) {
                if let Some(arc) = state.arcs[start + k] {
                    let c = state.analysis.arc_criticality(arc, &options);
                    criticality[start + k] = c;
                    timing_total += c * f64::from(
                        self.delays
                            .between(driver, places.pin_position(placement, pin)),
                    );
                }
            }
        }
        cost.bb_weight *= 1.0 - self.tradeoff;
        if timing_total > 0.0 {
            cost.timing_weight = self.tradeoff * COST_SCALE / timing_total;
        }
        cost.criticality = criticality;
        cost
    }
}

/// The criticality exponent for a range limit, as VPR's `update_td_costs` has it: from the
/// first exponent at the largest range limit to the last at a range of one step.
fn exponent(range: f64, max_range: f64, (first, last): (u32, u32)) -> u32 {
    let progress = if max_range > 1.0 {
        (1.0 - (range - 1.0) / (max_range - 1.0)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let value = f64::from(first) + progress * (f64::from(last) - f64::from(first));
    (value + 0.5) as u32
}

/// Places a design as VPR's placer weighs placements.
///
/// # Cost
///
/// VPR's, for `--place_algorithm criticality_timing`: each net's
/// [bounding box](PlaceProblem::bb_estimate), and each connection's delay by the
/// [`PlaceDelayModel`] times its criticality to an exponent, with weights that make each sum
/// `1 - timing_tradeoff` and `timing_tradeoff` of the whole for the placement so far. Each
/// temperature, the placement is timed again, with every connection at the model's delay, and
/// the criticalities and weights are worked out again with an exponent that grows as the range
/// limit shrinks, as VPR does. Without `timing`, or with a tradeoff of 0, the cost is the
/// bounding boxes alone, still weighed again each temperature.
///
/// # Annealing
///
/// [`Annealer`]'s, with VPR's schedule and moves, and median moves, with
/// [`inner_num`](PlacerOptions::inner_num). [`PlaceMethod::Anneal`] starts from a random legal
/// placement, as VPR does; [`PlaceMethod::Analytic`] from [`place_analytic`]'s legalized
/// placement, by plain wirelength, at a low temperature with a short range.
///
/// # Errors
///
/// [`PlaceError`] if the problem has more blocks of a type than sites for them, or a chain
/// finds no room.
pub fn place_design(
    problem: &PlaceProblem,
    delays: &PlaceDelayModel,
    timing: Option<(&mut DesignTiming, &Constraints)>,
    options: &PlacerOptions,
) -> Result<PlaceResult, PlaceError> {
    let places = &problem.problem;
    places.validate()?;
    let mut anneal_options = AnnealOptions {
        seed: options.seed,
        inner_num: options.inner_num,
        ..AnnealOptions::default()
    };
    let start = match options.method {
        PlaceMethod::Anneal => {
            let mut rng = Rng::new(options.seed ^ 0x5EED_F00D_CAFE_D00D);
            let extent = places.extent();
            let targets: Vec<Point> = (0..places.items().len())
                .map(|_| {
                    Point::new(
                        extent.min.x + rng.below(extent.width() + 1) as i64,
                        extent.min.y + rng.below(extent.height() + 1) as i64,
                    )
                })
                .collect();
            legalize(places, &targets, &vec![0; targets.len()])?
        }
        PlaceMethod::Analytic => {
            let analytic = AnalyticOptions {
                seed: options.seed,
                detailed: None,
                ..AnalyticOptions::default()
            };
            anneal_options.initial_temperature_factor = options.detailed_temperature;
            anneal_options.initial_range = Some(options.detailed_range);
            place_analytic(places, Wirelength, analytic)?.0.placement
        }
    };
    let start_bb_estimate = problem.bb_estimate(&start);

    let mut weigher = Weigher::new(problem, delays, timing, options.timing_tradeoff);
    let (first, last) = options.criticality_exponents;
    let cost = weigher.cost(&start, first);
    let mut annealer = Annealer::new(places, cost, start, anneal_options)?;
    while annealer.step().is_some() {
        if annealer.stop_reason().is_some() {
            break;
        }
        let exponent = exponent(annealer.range(), annealer.max_range(), (first, last));
        let cost = weigher.cost(annealer.placement(), exponent);
        annealer.set_cost(cost);
    }
    let outcome = annealer.outcome();
    let worst_slack = weigher.analyse(&outcome.placement);
    Ok(PlaceResult {
        bb_estimate: problem.bb_estimate(&outcome.placement),
        start_bb_estimate,
        placement: outcome.placement,
        worst_slack,
        temperatures: outcome.temperatures,
        moves: outcome.moves,
    })
}

/// Times a placement with the delay model, as [`place_design`] times its own, and returns the
/// worst slack, to compare placements made elsewhere by the same measure.
pub fn estimate_slack(
    problem: &PlaceProblem,
    delays: &PlaceDelayModel,
    timing: &mut DesignTiming,
    constraints: &Constraints,
    placement: &crate::place::Placement,
) -> Option<i64> {
    let mut weigher = Weigher::new(problem, delays, Some((timing, constraints)), 1.0);
    weigher.analyse(placement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_counts_are_vprs() {
        assert_eq!(crossing_count(2), 1.0);
        assert_eq!(crossing_count(3), 1.0);
        assert_eq!(crossing_count(4), 1.0828);
        assert_eq!(crossing_count(50), 2.7933);
        assert!((crossing_count(60) - (2.7933 + 0.2616)).abs() < 1e-12);
    }

    #[test]
    fn exponents_grow_as_the_range_shrinks() {
        assert_eq!(exponent(20.0, 20.0, (1, 8)), 1);
        assert_eq!(exponent(1.0, 20.0, (1, 8)), 8);
        assert_eq!(exponent(10.5, 20.0, (1, 8)), 5);
        assert_eq!(exponent(3.0, 1.0, (1, 8)), 8);
    }

    #[test]
    fn corners_are_picked_once_each() {
        assert_eq!(pick_corners(&[(1, 1)]), [(1, 1)]);
        let tiles: Vec<(u32, u32)> = (1..4).flat_map(|x| (1..4).map(move |y| (x, y))).collect();
        assert_eq!(
            pick_corners(&tiles),
            [(1, 1), (3, 1), (1, 3), (3, 3), (2, 2)]
        );
    }
}
