//! What the placers derive from a problem: which nets each item is on, which items move
//! together, and the sites of each kind as a grid.

use std::collections::BTreeMap;

use super::{ItemId, Layout, PlacementProblem, Point, SiteId};
use crate::id::NetId;

/// Something that moves as one: a movable item on its own, or a macro.
#[derive(Clone, Debug)]
pub(crate) struct Unit {
    /// The members and their origins' offsets from the unit's anchor. A single item is at
    /// offset zero, and a macro's anchor is where its offsets are measured from.
    pub members: Vec<(ItemId, Point)>,
    /// Whether this is a macro, whose members never rotate.
    pub is_macro: bool,
}

/// The sites of one kind, as a grid of their distinct columns and rows.
///
/// Range limits count steps in this grid, as VPR's compressed grids do, so that a kind that
/// only exists in some columns moves as freely as one that is everywhere.
#[derive(Clone, Debug)]
pub(crate) struct KindGrid {
    pub xs: Vec<i64>,
    pub ys: Vec<i64>,
    cell_start: Vec<u32>,
    cell_sites: Vec<SiteId>,
}

impl KindGrid {
    pub fn cell(&self, cx: usize, cy: usize) -> &[SiteId] {
        let c = cx * self.ys.len() + cy;
        &self.cell_sites[self.cell_start[c] as usize..self.cell_start[c + 1] as usize]
    }

    /// The column whose coordinate is nearest to `x`, the lower on a tie.
    pub fn nearest_column(&self, x: i64) -> usize {
        nearest(&self.xs, x)
    }

    /// The row whose coordinate is nearest to `y`, the lower on a tie.
    pub fn nearest_row(&self, y: i64) -> usize {
        nearest(&self.ys, y)
    }

    /// The column and row of exactly this point, if sites of this kind have it.
    pub fn locate(&self, at: Point) -> Option<(usize, usize)> {
        Some((
            self.xs.binary_search(&at.x).ok()?,
            self.ys.binary_search(&at.y).ok()?,
        ))
    }
}

fn nearest(values: &[i64], v: i64) -> usize {
    match values.binary_search(&v) {
        Ok(i) => i,
        Err(0) => 0,
        Err(i) if i >= values.len() => values.len() - 1,
        Err(i) => {
            if v - values[i - 1] <= values[i] - v {
                i - 1
            } else {
                i
            }
        }
    }
}

/// The derived view of a problem.
#[derive(Clone, Debug)]
pub(crate) struct Netlist {
    /// The nets of each item, each once, in order.
    item_net_start: Vec<u32>,
    item_nets: Vec<NetId>,
    /// The movable units.
    pub units: Vec<Unit>,
    /// The unit of each item, or `u32::MAX` for a fixed item.
    pub unit_of: Vec<u32>,
    /// For each site, its column, row and position among the sites of its cell.
    pub site_cell: Vec<(u32, u32, u32)>,
    /// The grid of each kind, by the kind's index in `kinds`.
    pub grids: Vec<KindGrid>,
    /// The kinds that have sites, in increasing order.
    pub kinds: Vec<u32>,
}

pub(crate) const NO_UNIT: u32 = u32::MAX;

impl Netlist {
    /// Derives the view. The problem must have passed `validate`.
    pub fn new(problem: &PlacementProblem) -> Netlist {
        let n = problem.items().len();
        let mut lists: Vec<Vec<NetId>> = vec![Vec::new(); n];
        for net in 0..problem.net_count() as u32 {
            for pin in problem.net_pins(NetId(net)) {
                let list = &mut lists[pin.item.index()];
                if list.last() != Some(&NetId(net)) {
                    list.push(NetId(net));
                }
            }
        }
        let mut item_net_start = Vec::with_capacity(n + 1);
        let mut item_nets = Vec::new();
        item_net_start.push(0);
        for list in lists {
            item_nets.extend(list);
            item_net_start.push(item_nets.len() as u32);
        }

        let macro_of = problem.macro_of();
        let mut units = Vec::new();
        let mut unit_of = vec![NO_UNIT; n];
        for members in problem.macros() {
            for &(item, _) in members {
                unit_of[item.index()] = units.len() as u32;
            }
            units.push(Unit {
                members: members.clone(),
                is_macro: true,
            });
        }
        for (i, item) in problem.items().iter().enumerate() {
            if item.is_movable() && macro_of[i].is_none() {
                unit_of[i] = units.len() as u32;
                units.push(Unit {
                    members: vec![(ItemId(i as u32), Point::default())],
                    is_macro: false,
                });
            }
        }

        let (site_cell, grids, kinds) = match problem.layout() {
            Layout::Sites(sites) => {
                let mut by_kind: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
                for (s, site) in sites.iter().enumerate() {
                    by_kind.entry(site.kind).or_default().push(s);
                }
                let mut site_cell = vec![(0, 0, 0); sites.len()];
                let mut grids = Vec::new();
                let mut kinds = Vec::new();
                for (kind, members) in by_kind {
                    let mut xs: Vec<i64> = members.iter().map(|&s| sites[s].at.x).collect();
                    let mut ys: Vec<i64> = members.iter().map(|&s| sites[s].at.y).collect();
                    xs.sort_unstable();
                    xs.dedup();
                    ys.sort_unstable();
                    ys.dedup();
                    let cells = xs.len() * ys.len();
                    let mut counts = vec![0u32; cells + 1];
                    let cell_of = |s: usize| {
                        let cx = xs.binary_search(&sites[s].at.x).unwrap_or(0);
                        let cy = ys.binary_search(&sites[s].at.y).unwrap_or(0);
                        (cx, cy, cx * ys.len() + cy)
                    };
                    for &s in &members {
                        counts[cell_of(s).2 + 1] += 1;
                    }
                    for c in 0..cells {
                        counts[c + 1] += counts[c];
                    }
                    let mut fill = counts.clone();
                    let mut cell_sites = vec![SiteId(0); members.len()];
                    for &s in &members {
                        let (cx, cy, c) = cell_of(s);
                        let sub = fill[c] - counts[c];
                        cell_sites[fill[c] as usize] = SiteId(s as u32);
                        fill[c] += 1;
                        site_cell[s] = (cx as u32, cy as u32, sub);
                    }
                    grids.push(KindGrid {
                        xs,
                        ys,
                        cell_start: counts,
                        cell_sites,
                    });
                    kinds.push(kind);
                }
                (site_cell, grids, kinds)
            }
            Layout::FreeSpace { .. } => (Vec::new(), Vec::new(), Vec::new()),
        };

        Netlist {
            item_net_start,
            item_nets,
            units,
            unit_of,
            site_cell,
            grids,
            kinds,
        }
    }

    /// The nets an item is on.
    pub fn nets_of(&self, item: ItemId) -> &[NetId] {
        let i = item.index();
        &self.item_nets[self.item_net_start[i] as usize..self.item_net_start[i + 1] as usize]
    }

    /// The index of a kind's grid, if the kind has sites.
    pub fn grid_of(&self, kind: u32) -> Option<usize> {
        self.kinds.binary_search(&kind).ok()
    }
}
