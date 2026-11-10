//! Incremental placement, after an engineering change: new items placed, and the rest kept
//! where they were unless it is worth moving them.

use super::anneal::{AnnealOptions, Annealer, PlaceOutcome};
use super::cost::{CostTerm, NetView};
use super::legalize::{
    legalize_units, nearest_free_spot, nearest_sites, site_unit_fits, typical_size, unit_box,
    unit_fits,
};
use super::netlist::{Netlist, Unit};
use super::problem::rotate;
use super::spatial::SpatialIndex;
use super::{Fixed, ItemId, Layout, PlaceError, Placement, PlacementProblem, Point, SiteId};
use crate::id::NetId;

/// Where an item was before a change: its origin and rotation, and on sites, its site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Prior {
    /// The origin.
    pub origin: Point,
    /// The rotation, in millidegrees.
    pub rotation: i32,
    /// The site, on a problem laid out in sites, which wins over `origin` while it is still a
    /// free site of the item's kind.
    pub site: Option<SiteId>,
}

impl Prior {
    /// Where `item` is in `placement`.
    pub fn of(placement: &Placement, item: ItemId) -> Prior {
        Prior {
            origin: placement.origin(item),
            rotation: placement.rotation(item),
            site: placement.site(item),
        }
    }

    /// Where every item of `placement` is, for a problem whose items are the same.
    pub fn all(placement: &Placement) -> Vec<Option<Prior>> {
        (0..placement.len())
            .map(|item| Some(Prior::of(placement, ItemId(item as u32))))
            .collect()
    }
}

/// A cost for moving items from where they were: for each item with a [`Prior`], its distance
/// from there, times `permille / 1000`.
///
/// It is a [`CostTerm`], so it is paid net by net: each item is charged through one of its
/// nets, the first, as the distance of its first pin on that net from where that pin was. That
/// is the distance its origin moved, unless it also turned. An item on no net is not charged,
/// and [`place_incremental`] cannot hold it in place. Add it to a [`Wirelength`] or any other
/// term as the pair `(term, displacement)`.
///
/// [`Wirelength`]: super::Wirelength
///
/// # Examples
///
/// ```
/// use guide::place::{Displacement, Item, Layout, Pin, Placement, PlacementProblem, Point, Prior, Rect};
///
/// let area = Rect::with_size(0, 0, 10, 10);
/// let mut problem = PlacementProblem::new(Layout::FreeSpace { area, pitch: 1 });
/// let a = problem.add_item(Item::new());
/// let b = problem.add_item(Item::new());
/// problem.add_net(1, &[Pin::new(a), Pin::new(b)]);
/// let mut was = Placement::new(&problem);
/// was.set_origin(b, Point::new(3, 0), 0);
///
/// // Only `b` was placed before.
/// let priors = [None, Some(Prior::of(&was, b))];
/// let displacement = Displacement::new(&problem, &priors, 500);
/// assert_eq!(displacement.items(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Displacement {
    permille: u64,
    /// By net, where its charged pins start in `pins`, and one more at the end.
    starts: Vec<u32>,
    /// The position in its net of each charged pin, and where it was.
    pins: Vec<(u32, Point)>,
}

impl Displacement {
    /// Charges `permille / 1000` per unit of distance for moving each item of `problem` that has
    /// a prior, by item, in `priors`. Fixed items and items beyond the end of `priors` are not
    /// charged.
    pub fn new(problem: &PlacementProblem, priors: &[Option<Prior>], permille: u64) -> Self {
        let mut charged = vec![false; problem.items().len()];
        let mut by_net: Vec<Vec<(u32, Point)>> = vec![Vec::new(); problem.net_count()];
        for (net, list) in by_net.iter_mut().enumerate() {
            for (position, pin) in problem.net_pins(NetId(net as u32)).iter().enumerate() {
                let item = pin.item.index();
                let Some(Some(prior)) = priors.get(item) else {
                    continue;
                };
                if charged[item] || !problem.items()[item].is_movable() {
                    continue;
                }
                charged[item] = true;
                let was = prior.origin.offset(rotate(pin.offset, prior.rotation));
                list.push((position as u32, was));
            }
        }
        let mut starts = Vec::with_capacity(by_net.len() + 1);
        let mut pins = Vec::new();
        starts.push(0);
        for list in by_net {
            pins.extend(list);
            starts.push(pins.len() as u32);
        }
        Displacement {
            permille,
            starts,
            pins,
        }
    }

    /// How many items are charged for moving.
    pub fn items(&self) -> usize {
        self.pins.len()
    }
}

impl CostTerm for Displacement {
    #[inline]
    fn net_cost(&self, net: &NetView<'_>) -> u64 {
        let index = net.net.index();
        let (Some(&start), Some(&end)) = (self.starts.get(index), self.starts.get(index + 1))
        else {
            return 0;
        };
        let distance = self.pins[start as usize..end as usize]
            .iter()
            .filter_map(|&(position, was)| {
                net.pins.get(position as usize).map(|&at| at.manhattan(was))
            })
            .fold(0u64, u64::saturating_add);
        distance.saturating_mul(self.permille) / 1000
    }
}

/// What [`place_incremental`] may do.
///
/// Create this with [`Default`] and set the fields.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct IncrementalOptions<'c> {
    /// What moving an item costs per unit of distance, in thousandths of what a unit of the
    /// cost term's wirelength costs: the [`Displacement`] term's factor. The default is 1000,
    /// so that moving an item one step has to shorten its nets by a step to be worth it. 0
    /// leaves items free to move as far as annealing takes them.
    pub displacement_permille: u64,
    /// The annealing that improves the placement once it is legal, or `None` for none. The
    /// default is [`AnnealOptions::detailed`] with an [`inner_num`](AnnealOptions::inner_num)
    /// of 1, a third of what detailed placement after analytic placement does.
    pub anneal: Option<AnnealOptions<'c>>,
}

impl Default for IncrementalOptions<'_> {
    fn default() -> Self {
        IncrementalOptions {
            displacement_permille: 1000,
            anneal: Some(AnnealOptions {
                inner_num: 1.0,
                ..AnnealOptions::detailed()
            }),
        }
    }
}

/// How far the items of a placement moved from their priors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PlacementChange {
    /// Movable items that have a prior, which are the ones compared.
    pub compared: u32,
    /// Of those, items whose origin or rotation is not what it was.
    pub moved: u32,
    /// The sum over those of the Manhattan distance from their prior origin.
    pub total_displacement: u64,
    /// The most that any item moved.
    pub max_displacement: u64,
    /// Movable items that have no prior, as items added.
    pub added: u32,
}

impl PlacementChange {
    /// The fraction of the items compared that moved, 0 if none were compared.
    pub fn moved_fraction(&self) -> f64 {
        if self.compared == 0 {
            0.0
        } else {
            f64::from(self.moved) / f64::from(self.compared)
        }
    }

    /// The mean displacement over the items compared, moved or not.
    pub fn mean_displacement(&self) -> f64 {
        if self.compared == 0 {
            0.0
        } else {
            self.total_displacement as f64 / f64::from(self.compared)
        }
    }
}

/// Measures how far the movable items of `placement` are from their priors, by item, in
/// `priors`. Items beyond the end of `priors` count as added.
///
/// # Examples
///
/// ```
/// use guide::place::{Item, Layout, Placement, PlacementProblem, Point, Prior, Rect, placement_change};
///
/// let area = Rect::with_size(0, 0, 10, 10);
/// let mut problem = PlacementProblem::new(Layout::FreeSpace { area, pitch: 1 });
/// let a = problem.add_item(Item::new());
/// let b = problem.add_item(Item::new());
/// let before = Placement::new(&problem);
/// let mut after = before.clone();
/// after.set_origin(b, Point::new(2, 1), 0);
///
/// let change = placement_change(&problem, &Prior::all(&before), &after);
/// assert_eq!((change.compared, change.moved, change.total_displacement), (2, 1, 3));
/// ```
pub fn placement_change(
    problem: &PlacementProblem,
    priors: &[Option<Prior>],
    placement: &Placement,
) -> PlacementChange {
    let mut change = PlacementChange::default();
    for (index, item) in problem.items().iter().enumerate() {
        if !item.is_movable() {
            continue;
        }
        let Some(Some(prior)) = priors.get(index) else {
            change.added += 1;
            continue;
        };
        let id = ItemId(index as u32);
        change.compared += 1;
        let distance = placement.origin(id).manhattan(prior.origin);
        if distance > 0 || placement.rotation(id) != prior.rotation {
            change.moved += 1;
        }
        change.total_displacement = change.total_displacement.saturating_add(distance);
        change.max_displacement = change.max_displacement.max(distance);
    }
    change
}

/// The result of [`place_incremental`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct IncrementalOutcome {
    /// The placement, which is legal, with its cost under the cost term and the displacement
    /// term together, and its wirelength.
    pub outcome: PlaceOutcome,
    /// How far it moved items from their priors.
    pub change: PlacementChange,
    /// Units, single items or macros, that legalization left where their priors put them.
    pub kept_by_legalization: u32,
    /// Units that legalization placed afresh: those with no prior, and those whose prior
    /// place was taken, gone or no longer legal.
    pub placed_by_legalization: u32,
}

/// Places a problem that has changed since it was placed, keeping items where they were unless
/// it is worth moving them.
///
/// `priors` has, by item, where each item was before the change, or `None` for an item that is
/// new. An item whose identity changed with the change, as when items were renumbered, should
/// have its old place here, which [`restrict`] helps with.
///
/// 1. **Legalization that moves as little as it can.** Each unit, a single item or a macro,
///    whose members all have priors, goes back where they put it if that is still legal: a free
///    site of the item's kind, its prior site first, or a spot where its outline overlaps
///    nothing so far. The rest go, in the order of [`legalize`](super::legalize), to the free
///    place nearest to their target: their prior place, or for a new item the median of the
///    places of the items it shares nets with, found in rounds so that new items connected only
///    to new items follow, or the middle of its region or of the layout. Only if that finds no
///    room are all units legalized again, with the same targets.
/// 2. **Local improvement.** The [`Annealer`], with [`options.anneal`](IncrementalOptions), on
///    `cost` plus a [`Displacement`] term that charges for moving items with priors, so that an
///    item moves only where its nets gain more than the move costs. Items with priors keep
///    their rotation, which a displacement measured on pins would not always see: only new
///    items choose theirs.
///
/// This is deterministic, as every placer here is.
///
/// # Errors
///
/// [`PlaceError::Problem`] if the problem is malformed, and [`PlaceError::NoRoom`] if
/// legalization finds no room for an item.
///
/// # Examples
///
/// A grid placed, then given one more item, which goes next to what it connects to while the
/// rest stay where they were.
///
/// ```
/// use guide::place::{
///     AnalyticOptions, IncrementalOptions, Item, Layout, Pin, PlacementProblem, Point, Prior,
///     Site, Wirelength, check, place_analytic, place_incremental,
/// };
///
/// let sites = (0..5).flat_map(|x| (0..5).map(move |y| Site::new(Point::new(x, y), 0)));
/// let mut problem = PlacementProblem::new(Layout::Sites(sites.collect()));
/// let items: Vec<_> = (0..12).map(|_| problem.add_item(Item::new())).collect();
/// for pair in items.windows(2) {
///     problem.add_net(1, &[Pin::new(pair[0]), Pin::new(pair[1])]);
/// }
/// let (before, _) = place_analytic(&problem, &Wirelength, AnalyticOptions::default())?;
///
/// let mut changed = problem.clone();
/// let new = changed.add_item(Item::new());
/// changed.add_net(1, &[Pin::new(items[5]), Pin::new(new)]);
/// let mut priors = Prior::all(&before.placement);
/// priors.push(None);
///
/// let after = place_incremental(&changed, &Wirelength, &priors, IncrementalOptions::default())?;
/// assert!(check(&changed, &after.outcome.placement).is_empty());
/// assert_eq!(after.change.added, 1);
/// assert!(after.change.moved_fraction() < 0.5);
/// # Ok::<(), guide::place::PlaceError>(())
/// ```
pub fn place_incremental<C: CostTerm>(
    problem: &PlacementProblem,
    cost: C,
    priors: &[Option<Prior>],
    options: IncrementalOptions<'_>,
) -> Result<IncrementalOutcome, PlaceError> {
    problem.validate()?;
    let netlist = Netlist::new(problem);
    let prior = |item: ItemId| priors.get(item.index()).copied().flatten();
    let targets = targets(problem, &netlist, &prior);
    let (legal, kept) = legalize_keeping(problem, &netlist, &prior, &targets)?;
    let placed = netlist.units.len() as u32 - kept;
    let displacement = Displacement::new(problem, priors, options.displacement_permille);
    let held = (0..problem.items().len())
        .map(|item| prior(ItemId(item as u32)).is_some())
        .collect();
    let mut annealer = Annealer::with_netlist(
        problem,
        (cost, displacement),
        legal,
        options.anneal.unwrap_or_default(),
        netlist,
    );
    annealer.hold_rotations(held);
    let outcome = match options.anneal {
        Some(_) => annealer.run(),
        None => {
            let mut outcome = annealer.outcome();
            outcome.stop = super::PlaceStop::Converged;
            outcome
        }
    };
    let change = placement_change(problem, priors, &outcome.placement);
    Ok(IncrementalOutcome {
        outcome,
        change,
        kept_by_legalization: kept,
        placed_by_legalization: placed,
    })
}

/// How many rounds new items' targets are worked out in, each from the items placed or targeted
/// before it.
const TARGET_ROUNDS: usize = 4;

/// Where each item would like to be: its prior origin, where it is fixed, or for a new item,
/// the median of where the items it shares nets with are.
fn targets(
    problem: &PlacementProblem,
    netlist: &Netlist,
    prior: &impl Fn(ItemId) -> Option<Prior>,
) -> Vec<Option<Point>> {
    let fresh = Placement::new(problem);
    let mut known: Vec<Option<Point>> = problem
        .items()
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let id = ItemId(index as u32);
            match item.fixed() {
                Some(_) => Some(fresh.origin(id)),
                None => prior(id).map(|p| p.origin),
            }
        })
        .collect();
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for _ in 0..TARGET_ROUNDS {
        let before = known.clone();
        let mut progress = false;
        for index in 0..known.len() {
            if before[index].is_some() {
                continue;
            }
            xs.clear();
            ys.clear();
            for &net in netlist.nets_of(ItemId(index as u32)) {
                for pin in problem.net_pins(net) {
                    if let Some(at) = before[pin.item.index()] {
                        xs.push(at.x);
                        ys.push(at.y);
                    }
                }
            }
            if xs.is_empty() {
                continue;
            }
            xs.sort_unstable();
            ys.sort_unstable();
            known[index] = Some(Point::new(xs[xs.len() / 2], ys[ys.len() / 2]));
            progress = true;
        }
        if !progress {
            break;
        }
    }
    known
}

/// The target of an item that has none: the middle of its region, or of the layout.
fn middle(problem: &PlacementProblem, item: ItemId) -> Point {
    let area = problem.item(item).limited_to().map_or_else(
        || problem.extent(),
        |region| problem.regions()[region.index()],
    );
    Point::new(
        area.min.x + (area.max.x - area.min.x) / 2,
        area.min.y + (area.max.y - area.min.y) / 2,
    )
}

/// Legalizes with units whose members all have priors left there when that is legal, and the
/// others put as near their targets as there is room. Returns the placement and how many units
/// stayed where their priors put them.
fn legalize_keeping(
    problem: &PlacementProblem,
    netlist: &Netlist,
    prior: &impl Fn(ItemId) -> Option<Prior>,
    targets: &[Option<Point>],
) -> Result<(Placement, u32), PlaceError> {
    let units = &netlist.units;
    let anchor_of = |unit: &Unit| {
        let (first, offset) = unit.members[0];
        targets[first.index()]
            .unwrap_or_else(|| middle(problem, first))
            .minus(offset)
    };
    let anchors: Vec<Point> = units.iter().map(anchor_of).collect();
    let rotations: Vec<i32> = units
        .iter()
        .map(|unit| {
            let item = unit.members[0].0;
            let allowed = problem.item(item).allowed_rotations();
            match prior(item) {
                Some(p) if !unit.is_macro && allowed.contains(&p.rotation) => p.rotation,
                _ => allowed[0],
            }
        })
        .collect();
    let whole = |unit: &Unit| unit.members.iter().all(|&(item, _)| prior(item).is_some());
    let mut placement = Placement::new(problem);
    let mut kept = 0u32;
    let mut pending: Vec<usize> = Vec::new();

    match problem.layout() {
        Layout::Sites(sites) => {
            let mut taken = vec![false; sites.len()];
            for item in problem.items() {
                if let Some(Fixed::Site(site)) = item.fixed() {
                    taken[site.index()] = true;
                }
            }
            let mut members = Vec::new();
            for (u, unit) in units.iter().enumerate() {
                let (first, offset) = unit.members[0];
                let start = whole(unit)
                    .then(|| prior(first))
                    .flatten()
                    .and_then(|p| prior_site(problem, netlist, first, p, &taken));
                let fits = start.is_some_and(|site| {
                    let anchor = sites[site.index()].at.minus(offset);
                    site_unit_fits(problem, netlist, unit, site, anchor, &taken, &mut members)
                });
                if !fits {
                    pending.push(u);
                    continue;
                }
                kept += 1;
                for (&(item, _), &site) in unit.members.iter().zip(&members) {
                    taken[site.index()] = true;
                    placement.set_site(problem, item, site);
                    placement.rotations[item.index()] = if unit.is_macro {
                        problem.item(item).allowed_rotations()[0]
                    } else {
                        rotations[u]
                    };
                }
            }
            // As `legalize` orders them: macros first, then those limited to the smallest
            // region, then from left to right.
            pending.sort_by_key(|&u| {
                let unit = &units[u];
                (
                    !unit.is_macro,
                    room(problem, unit),
                    anchors[u].x,
                    anchors[u].y,
                    u,
                )
            });
            for &u in &pending {
                let unit = &units[u];
                let Some(chosen) = nearest_sites(problem, netlist, unit, anchors[u], &taken) else {
                    let legal = legalize_units(problem, netlist, &anchors, &rotations)?;
                    return Ok((legal, 0));
                };
                for (&(item, _), site) in unit.members.iter().zip(chosen) {
                    taken[site.index()] = true;
                    placement.set_site(problem, item, site);
                    placement.rotations[item.index()] = if unit.is_macro {
                        problem.item(item).allowed_rotations()[0]
                    } else {
                        rotations[u]
                    };
                }
            }
        }
        Layout::FreeSpace { area, pitch } => {
            let mut index = SpatialIndex::new(*area, problem.items().len(), typical_size(problem));
            for (i, item) in problem.items().iter().enumerate() {
                if item.fixed().is_some() {
                    let id = ItemId(i as u32);
                    let outline =
                        problem.outline_at(id, placement.origins[i], placement.rotations[i]);
                    index.insert(i as u32, outline);
                }
            }
            let put = |placement: &mut Placement, index: &mut SpatialIndex, u: usize, at: Point| {
                let unit = &units[u];
                for &(item, offset) in &unit.members {
                    let rotation = if unit.is_macro {
                        problem.item(item).allowed_rotations()[0]
                    } else {
                        rotations[u]
                    };
                    let origin = at.offset(offset);
                    placement.set_origin(item, origin, rotation);
                    index.insert(item.0, problem.outline_at(item, origin, rotation));
                }
            };
            for (u, unit) in units.iter().enumerate() {
                let (first, offset) = unit.members[0];
                let at = whole(unit)
                    .then(|| prior(first))
                    .flatten()
                    .map(|p| p.origin.minus(offset));
                match at {
                    Some(at) if unit_fits(problem, unit, rotations[u], at, area, &index) => {
                        kept += 1;
                        put(&mut placement, &mut index, u, at);
                    }
                    _ => pending.push(u),
                }
            }
            // Largest first, as `legalize` does.
            pending.sort_by_key(|&u| {
                let unit = &units[u];
                let size = unit_box(problem, unit, rotations[u]).area();
                (
                    room(problem, unit),
                    u64::MAX - size,
                    anchors[u].x,
                    anchors[u].y,
                    u,
                )
            });
            for &u in &pending {
                let unit = &units[u];
                let spot = nearest_free_spot(
                    problem,
                    unit,
                    rotations[u],
                    anchors[u],
                    *area,
                    *pitch,
                    &index,
                );
                let Some(at) = spot else {
                    let legal = legalize_units(problem, netlist, &anchors, &rotations)?;
                    return Ok((legal, 0));
                };
                put(&mut placement, &mut index, u, at);
            }
        }
    }
    Ok((placement, kept))
}

/// The area of the smallest region a unit's members are limited to.
fn room(problem: &PlacementProblem, unit: &Unit) -> u64 {
    unit.members
        .iter()
        .filter_map(|&(item, _)| problem.item(item).limited_to())
        .map(|region| problem.regions()[region.index()].area())
        .min()
        .unwrap_or(u64::MAX)
}

/// The site an item with a prior goes back to, if one is free: its prior site, or else a site
/// of its kind at its prior origin.
fn prior_site(
    problem: &PlacementProblem,
    netlist: &Netlist,
    item: ItemId,
    prior: Prior,
    taken: &[bool],
) -> Option<SiteId> {
    let sites = problem.sites();
    let kind = problem.item(item).site_kind();
    let usable = |site: SiteId| {
        sites
            .get(site.index())
            .is_some_and(|s| s.kind == kind && !taken[site.index()])
    };
    if let Some(site) = prior.site.filter(|&site| usable(site))
        && sites[site.index()].at == prior.origin
    {
        return Some(site);
    }
    let grid = &netlist.grids[netlist.grid_of(kind)?];
    let (x, y) = grid.locate(prior.origin)?;
    grid.cell(x, y).iter().copied().find(|&site| usable(site))
}

/// A problem with some of the items and nets of another taken out, and where each item of the
/// other went in it.
///
/// This is the inverse of an engineering change that adds items and nets, which is how
/// incremental placement is measured: place the restricted problem, then the whole one with
/// [`place_incremental`], from priors that [`Restricted::priors`] maps back.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Restricted {
    /// The problem, with the items and nets kept in their order, and pins on items taken out
    /// left out of their nets.
    pub problem: PlacementProblem,
    /// By item of the original problem, its item in this one, if it was kept.
    pub item_map: Vec<Option<ItemId>>,
}

impl Restricted {
    /// The priors, by item of the original problem, that a placement of the restricted problem
    /// gives: where each kept item is, and `None` for the others.
    pub fn priors(&self, placement: &Placement) -> Vec<Option<Prior>> {
        self.item_map
            .iter()
            .map(|item| item.map(|item| Prior::of(placement, item)))
            .collect()
    }
}

/// Takes items and nets out of a problem: see [`Restricted`].
///
/// A macro with a member taken out loses all of its members' ties, and those kept are single
/// items. Regions are kept as they are.
///
/// # Panics
///
/// If `keep_item` has fewer entries than the problem has items, or `keep_net` than it has nets.
pub fn restrict(problem: &PlacementProblem, keep_item: &[bool], keep_net: &[bool]) -> Restricted {
    let mut out = PlacementProblem::new(problem.layout().clone());
    for &region in problem.regions() {
        out.add_region(region);
    }
    let mut item_map = vec![None; problem.items().len()];
    for (index, item) in problem.items().iter().enumerate() {
        if keep_item[index] {
            item_map[index] = Some(out.add_item(item.clone()));
        }
    }
    let mut pins = Vec::new();
    for (net, &keep) in keep_net[..problem.net_count()].iter().enumerate() {
        if !keep {
            continue;
        }
        let id = NetId(net as u32);
        pins.clear();
        for pin in problem.net_pins(id) {
            if let Some(item) = item_map[pin.item.index()] {
                pins.push(super::Pin {
                    item,
                    offset: pin.offset,
                });
            }
        }
        out.add_net(problem.net_weight(id), &pins);
    }
    for members in problem.macros() {
        let kept: Option<Vec<(ItemId, Point)>> = members
            .iter()
            .map(|&(item, offset)| item_map[item.index()].map(|item| (item, offset)))
            .collect();
        if let Some(kept) = kept {
            out.add_macro(&kept);
        }
    }
    Restricted {
        problem: out,
        item_map,
    }
}
