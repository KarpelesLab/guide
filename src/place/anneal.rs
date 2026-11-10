//! Simulated annealing, after VPR.

use core::sync::atomic::{AtomicBool, Ordering};

use super::cost::{CostTerm, NetView};
use super::legalize::{legalize_units, snap, typical_size, unit_box, unit_fits};
use super::netlist::{NO_UNIT, Netlist};
use super::spatial::SpatialIndex;
use super::{ItemId, Layout, PlaceError, Placement, PlacementProblem, Point, Rect, SiteId, check};
use crate::id::NetId;
use crate::math::exp;
use crate::rng::Rng;

/// No item on a site.
const EMPTY: u32 = u32::MAX;

/// How many moves go by between reads of the cancel flag.
const CANCEL_EVERY: u64 = 1024;

/// The fewest moves tried at a temperature, so that tiny problems still settle.
const MIN_MOVES: f64 = 100.0;

/// What an [`Annealer`] may do, and when it has to stop.
///
/// Create this with [`Default`] or [`detailed`](AnnealOptions::detailed) and set the fields.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct AnnealOptions<'c> {
    /// The seed of every random choice. The default is 1.
    pub seed: u64,
    /// Moves per temperature, as a multiple of `units^(4/3)`. The default is 1, as VPR's
    /// `--inner_num`. More gives better results more slowly.
    pub inner_num: f64,
    /// The first temperature, as a multiple of the standard deviation of random moves' cost
    /// changes. The default is 20, as in VPR, which accepts nearly everything at first.
    pub initial_temperature_factor: f64,
    /// The first range limit, in steps. The default is `None`, the whole layout.
    pub initial_range: Option<u32>,
    /// Annealing ends when the temperature is below this times the average cost of a net.
    /// The default is 0.005, as in VPR.
    pub exit_factor: f64,
    /// How many moves may be tried in all, counting those that measure the first
    /// temperature. The default is no limit.
    pub max_moves: u64,
    /// A flag that stops the run once set, from any thread. The default is none.
    pub cancel: Option<&'c AtomicBool>,
}

impl Default for AnnealOptions<'_> {
    fn default() -> Self {
        AnnealOptions {
            seed: 1,
            inner_num: 1.0,
            initial_temperature_factor: 20.0,
            initial_range: None,
            exit_factor: 0.005,
            max_moves: u64::MAX,
            cancel: None,
        }
    }
}

impl AnnealOptions<'_> {
    /// Options for detailed placement: annealing that starts cold, over a few steps, to
    /// improve a placement that is already good without undoing it. The temperature starts
    /// at a twentieth of the standard deviation of moves within 3 steps.
    pub fn detailed() -> Self {
        AnnealOptions {
            initial_temperature_factor: 0.05,
            initial_range: Some(3),
            ..AnnealOptions::default()
        }
    }
}

/// Why an annealing run stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlaceStop {
    /// The schedule ran to its end.
    Converged,
    /// [`max_moves`](AnnealOptions::max_moves) ran out.
    BudgetExhausted,
    /// The [`cancel`](AnnealOptions::cancel) flag was set.
    Cancelled,
    /// The run hasn't stopped: its result was asked for before it finished.
    NotFinished,
}

/// What one temperature of an [`Annealer`] did.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TemperatureReport {
    /// The number of this temperature, from 1.
    pub index: u32,
    /// The temperature, 0 for the last pass.
    pub temperature: f64,
    /// The range limit, in steps.
    pub range: f64,
    /// Moves tried.
    pub moves: u64,
    /// Moves accepted.
    pub accepted: u64,
    /// The cost after it.
    pub cost: u64,
    /// The lowest cost at the end of any temperature so far.
    pub best_cost: u64,
}

/// The result of a placer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaceOutcome {
    /// The placement, which is legal.
    pub placement: Placement,
    /// Its cost under the cost term the placer was given.
    pub cost: u64,
    /// Its weighted half-perimeter wirelength.
    pub wirelength: u64,
    /// Why the placer stopped.
    pub stop: PlaceStop,
    /// How many annealing moves were tried.
    pub moves: u64,
    /// How many temperatures annealing went through.
    pub temperatures: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Start,
    Anneal,
    Quench,
}

/// An item's state before a move, to take the move back.
#[derive(Clone, Copy, Debug)]
struct Old {
    item: ItemId,
    origin: Point,
    rotation: i32,
    site: Option<SiteId>,
}

/// A simulated annealer, after VPR, run one temperature at a time.
///
/// # Moves
///
/// Each move picks a movable unit, an item or a macro, at random, and a place for it within
/// the range limit of where it is: a site of its kind at most that many columns and rows away,
/// counted among the columns and rows that have sites of the kind, or a point at most that many
/// pitches away in free space. If another single item is there, the two swap. On sites, a macro
/// that lands on single items sends each of them to the site a member left, following the
/// chain of a shift that overlaps itself, as VPR does. In free space, a macro only moves to
/// where nothing is. An item allowed several rotations is sometimes turned in place instead.
/// A move that would break legality, by leaving a region, overlapping or swapping with a
/// macro member or a fixed item, is abandoned and counts as rejected, so every placement the
/// annealer holds is legal.
///
/// # Cost
///
/// The cost is the sum over nets of a [`CostTerm`], kept per net. A move only asks again for
/// the nets of the items it moves, and is accepted if it lowers the cost, or otherwise with
/// probability `exp(-delta / T)`, through [`crate::math::exp`].
///
/// # Schedule
///
/// VPR's adaptive schedule:
///
/// - The first temperature is [`initial_temperature_factor`](AnnealOptions) times the standard
///   deviation of the cost changes of one random move per unit, each tried and taken back.
/// - Each temperature tries `inner_num * units^(4/3)` moves, and at least 100, which only
///   matters below about 30 units.
/// - After it, with `a` the fraction of moves accepted, the temperature is multiplied by 0.5
///   when `a > 0.96`, 0.9 when `a > 0.8`, 0.95 when `a > 0.15` or the range limit is above one
///   step, and 0.8 otherwise. The range limit is multiplied by `0.56 + a`, within one step and
///   the size of the layout, so that about 44% of moves are accepted.
/// - Annealing ends when the temperature falls below [`exit_factor`](AnnealOptions) times the
///   average cost of a net, and one last pass at temperature zero takes only moves that don't
///   raise the cost.
///
/// The best placement seen at the end of a temperature is kept, and that is what a run that
/// stops early returns.
///
/// # Examples
///
/// ```
/// use guide::place::{AnnealOptions, Annealer, Wirelength, check, legalize, synth};
///
/// let generated = synth::grid(4, 4, 7);
/// let problem = &generated.problem;
/// let start = legalize(problem, &vec![Default::default(); problem.items().len()], &vec![0; problem.items().len()])?;
/// let mut annealer = Annealer::new(problem, Wirelength, start, AnnealOptions::default())?;
/// while let Some(report) = annealer.step() {
///     assert!(report.best_cost <= report.cost || report.temperature > 0.0);
/// }
/// let outcome = annealer.outcome();
/// assert!(check(problem, &outcome.placement).is_empty());
/// assert!(outcome.wirelength >= generated.optimum);
/// # Ok::<(), guide::place::PlaceError>(())
/// ```
pub struct Annealer<'a, C> {
    problem: &'a PlacementProblem,
    cost: C,
    options: AnnealOptions<'a>,
    netlist: Netlist,
    rng: Rng,
    placement: Placement,
    occupant: Vec<u32>,
    index: Option<SpatialIndex>,
    area: Rect,
    pitch: i64,
    net_cost: Vec<u64>,
    total: u64,
    live_nets: u64,
    stamp: Vec<u32>,
    generation: u32,
    touched: Vec<NetId>,
    new_costs: Vec<u64>,
    pins: Vec<Point>,
    moved: Vec<Old>,
    proposal: Vec<(ItemId, Point, i32, Option<SiteId>)>,
    temperature: f64,
    range: f64,
    max_range: f64,
    moves_per_temperature: u64,
    phase: Phase,
    moves: u64,
    temperatures: u32,
    stop: Option<PlaceStop>,
    best: Placement,
    best_cost: u64,
    reports: Vec<TemperatureReport>,
}

impl<'a, C: CostTerm> Annealer<'a, C> {
    /// Prepares to anneal from `start`, which has to be legal.
    ///
    /// # Errors
    ///
    /// [`PlaceError::Problem`] if the problem is malformed, and [`PlaceError::IllegalStart`]
    /// if `start` is not legal.
    pub fn new(
        problem: &'a PlacementProblem,
        cost: C,
        start: Placement,
        options: AnnealOptions<'a>,
    ) -> Result<Self, PlaceError> {
        problem.validate()?;
        if let Some(violation) = check(problem, &start).into_iter().next() {
            return Err(PlaceError::IllegalStart(violation));
        }
        Ok(Self::with_netlist(
            problem,
            cost,
            start,
            options,
            Netlist::new(problem),
        ))
    }

    pub(crate) fn with_netlist(
        problem: &'a PlacementProblem,
        cost: C,
        start: Placement,
        options: AnnealOptions<'a>,
        netlist: Netlist,
    ) -> Self {
        let (occupant, index, area, pitch, max_range) = match problem.layout() {
            Layout::Sites(sites) => {
                let mut occupant = vec![EMPTY; sites.len()];
                for (i, site) in start.sites.iter().enumerate() {
                    if let Some(site) = site {
                        occupant[site.index()] = i as u32;
                    }
                }
                let reach = netlist
                    .grids
                    .iter()
                    .map(|g| g.xs.len().max(g.ys.len()))
                    .max()
                    .unwrap_or(1);
                (occupant, None, problem.extent(), 1, reach as f64)
            }
            Layout::FreeSpace { area, pitch } => {
                let mut index =
                    SpatialIndex::new(*area, problem.items().len(), typical_size(problem));
                for i in 0..problem.items().len() {
                    let id = ItemId(i as u32);
                    index.insert(
                        id.0,
                        problem.outline_at(id, start.origins[i], start.rotations[i]),
                    );
                }
                let reach = (area.width().max(area.height()) as i64 / pitch).max(1);
                (Vec::new(), Some(index), *area, *pitch, reach as f64)
            }
        };
        let units = netlist.units.len() as f64;
        let moves_per_temperature =
            (options.inner_num * units * cube_root(units)).max(MIN_MOVES) as u64;
        let range = options
            .initial_range
            .map_or(max_range, |r| f64::from(r.max(1)).min(max_range))
            .max(1.0);
        let mut annealer = Annealer {
            problem,
            cost,
            options,
            rng: Rng::new(options.seed),
            placement: start.clone(),
            occupant,
            index,
            area,
            pitch,
            net_cost: vec![0; problem.net_count()],
            total: 0,
            live_nets: 0,
            stamp: vec![0; problem.net_count()],
            generation: 0,
            touched: Vec::new(),
            new_costs: Vec::new(),
            pins: Vec::new(),
            moved: Vec::new(),
            proposal: Vec::new(),
            temperature: 0.0,
            range,
            max_range,
            moves_per_temperature,
            phase: Phase::Start,
            moves: 0,
            temperatures: 0,
            stop: None,
            best: start,
            best_cost: 0,
            reports: Vec::new(),
            netlist,
        };
        let mut total = 0u64;
        for net in 0..problem.net_count() {
            let c = annealer.compute_net(NetId(net as u32));
            annealer.net_cost[net] = c;
            total = total.saturating_add(c);
            if problem.net_pins(NetId(net as u32)).len() >= 2 {
                annealer.live_nets += 1;
            }
        }
        annealer.total = total;
        annealer.best_cost = total;
        if annealer.netlist.units.is_empty() {
            annealer.stop = Some(PlaceStop::Converged);
        }
        annealer
    }

    /// The current cost.
    pub fn cost(&self) -> u64 {
        self.total
    }

    /// The current placement, which is always legal.
    pub fn placement(&self) -> &Placement {
        &self.placement
    }

    /// Why the run stopped, or `None` if it hasn't.
    pub fn stop_reason(&self) -> Option<PlaceStop> {
        self.stop
    }

    /// Every temperature's report so far.
    pub fn reports(&self) -> &[TemperatureReport] {
        &self.reports
    }

    /// Runs one temperature and reports on it, or returns `None` once the run has stopped.
    pub fn step(&mut self) -> Option<TemperatureReport> {
        if self.stop.is_some() {
            return None;
        }
        if self.cancelled() {
            self.stop = Some(PlaceStop::Cancelled);
            return None;
        }
        if self.phase == Phase::Start {
            if !self.start_temperature() {
                return None;
            }
            self.phase = if self.temperature > 0.0 && self.total > 0 {
                Phase::Anneal
            } else {
                Phase::Quench
            };
        }
        let temperature = if self.phase == Phase::Quench {
            0.0
        } else {
            self.temperature
        };
        let mut tried = 0;
        let mut accepted = 0;
        while tried < self.moves_per_temperature {
            if self.moves >= self.options.max_moves {
                self.stop = Some(PlaceStop::BudgetExhausted);
                break;
            }
            if self.moves % CANCEL_EVERY == CANCEL_EVERY - 1 && self.cancelled() {
                self.stop = Some(PlaceStop::Cancelled);
                break;
            }
            self.moves += 1;
            tried += 1;
            if let Some(delta) = self.try_move() {
                let accept = delta <= 0
                    || (temperature > 0.0 && self.rng.unit() < exp(-(delta as f64) / temperature));
                if accept {
                    self.commit();
                    accepted += 1;
                } else {
                    self.revert();
                }
            }
        }
        self.temperatures += 1;
        if self.total < self.best_cost {
            self.best_cost = self.total;
            self.best.clone_from(&self.placement);
        }
        let report = TemperatureReport {
            index: self.temperatures,
            temperature,
            range: self.range,
            moves: tried,
            accepted,
            cost: self.total,
            best_cost: self.best_cost,
        };
        self.reports.push(report);
        if self.stop.is_some() {
            return Some(report);
        }
        match self.phase {
            Phase::Quench => self.stop = Some(PlaceStop::Converged),
            _ => {
                let rate = accepted as f64 / tried.max(1) as f64;
                self.update_schedule(rate);
                let average = self.total as f64 / self.live_nets.max(1) as f64;
                if self.temperature < self.options.exit_factor * average || self.total == 0 {
                    self.phase = Phase::Quench;
                }
            }
        }
        Some(report)
    }

    /// Runs to the end and returns the result.
    pub fn run(mut self) -> PlaceOutcome {
        while self.step().is_some() {}
        self.outcome()
    }

    /// The best placement so far, as a result. A run that hasn't stopped reports
    /// [`PlaceStop::NotFinished`].
    pub fn outcome(&self) -> PlaceOutcome {
        let (placement, cost) = if self.total <= self.best_cost {
            (self.placement.clone(), self.total)
        } else {
            (self.best.clone(), self.best_cost)
        };
        PlaceOutcome {
            wirelength: self.problem.wirelength(&placement),
            placement,
            cost,
            stop: self.stop.unwrap_or(PlaceStop::NotFinished),
            moves: self.moves,
            temperatures: self.temperatures,
        }
    }

    fn cancelled(&self) -> bool {
        self.options
            .cancel
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Measures the first temperature, or returns false if the run stopped while doing so.
    fn start_temperature(&mut self) -> bool {
        let samples = self.netlist.units.len();
        let mut sum = 0.0;
        let mut squares = 0.0;
        let mut count = 0u32;
        for _ in 0..samples {
            if self.moves >= self.options.max_moves {
                self.stop = Some(PlaceStop::BudgetExhausted);
                return false;
            }
            self.moves += 1;
            if let Some(delta) = self.try_move() {
                let d = delta as f64;
                sum += d;
                squares += d * d;
                count += 1;
                self.revert();
            }
        }
        let n = f64::from(count.max(1));
        let mean = sum / n;
        let variance = (squares / n - mean * mean).max(0.0);
        self.temperature = self.options.initial_temperature_factor * variance.sqrt();
        true
    }

    fn update_schedule(&mut self, rate: f64) {
        let factor = if rate > 0.96 {
            0.5
        } else if rate > 0.8 {
            0.9
        } else if rate > 0.15 || self.range > 1.0 {
            0.95
        } else {
            0.8
        };
        self.temperature *= factor;
        self.range = (self.range * (1.0 - 0.44 + rate)).clamp(1.0, self.max_range);
    }

    /// The cost of one net in the current placement.
    fn compute_net(&mut self, net: NetId) -> u64 {
        let pins = self.problem.net_pins(net);
        if pins.is_empty() {
            return 0;
        }
        self.pins.clear();
        let mut bounds: Option<Rect> = None;
        for &pin in pins {
            let at = self.problem.pin_position(&self.placement, pin);
            bounds = Some(bounds.map_or(Rect::point(at), |b| b.including(at)));
            self.pins.push(at);
        }
        self.cost.net_cost(&NetView {
            net,
            weight: self.problem.net_weight(net),
            pins: &self.pins,
            bounds: bounds.unwrap_or_default(),
        })
    }

    /// Proposes a random move, applies it and returns the change in cost, or returns `None`
    /// if the move was abandoned, leaving nothing applied.
    fn try_move(&mut self) -> Option<i128> {
        self.proposal.clear();
        let unit = self.rng.below(self.netlist.units.len() as u64) as usize;
        let ok = match self.problem.layout() {
            Layout::Sites(_) => self.propose_on_sites(unit),
            Layout::FreeSpace { .. } => self.propose_in_free_space(unit),
        };
        if !ok || self.proposal.is_empty() {
            return None;
        }
        // Apply.
        self.moved.clear();
        for &(item, origin, rotation, site) in &self.proposal {
            let i = item.index();
            self.moved.push(Old {
                item,
                origin: self.placement.origins[i],
                rotation: self.placement.rotations[i],
                site: self.placement.sites[i],
            });
            self.placement.origins[i] = origin;
            self.placement.rotations[i] = rotation;
            self.placement.sites[i] = site;
        }
        // Price the nets touched.
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.fill(0);
            self.generation = 1;
        }
        self.touched.clear();
        for m in 0..self.moved.len() {
            let item = self.moved[m].item;
            for &net in self.netlist.nets_of(item) {
                if self.stamp[net.index()] != self.generation {
                    self.stamp[net.index()] = self.generation;
                    self.touched.push(net);
                }
            }
        }
        self.new_costs.clear();
        let mut delta: i128 = 0;
        for t in 0..self.touched.len() {
            let net = self.touched[t];
            let c = self.compute_net(net);
            delta += i128::from(c) - i128::from(self.net_cost[net.index()]);
            self.new_costs.push(c);
        }
        Some(delta)
    }

    /// Keeps the move just tried.
    fn commit(&mut self) {
        for (t, &net) in self.touched.iter().enumerate() {
            let old = core::mem::replace(&mut self.net_cost[net.index()], self.new_costs[t]);
            self.total = self
                .total
                .saturating_sub(old)
                .saturating_add(self.new_costs[t]);
        }
        if let Some(index) = &mut self.index {
            for old in &self.moved {
                index.remove(old.item.0);
            }
            for old in &self.moved {
                let i = old.item.index();
                index.insert(
                    old.item.0,
                    self.problem.outline_at(
                        old.item,
                        self.placement.origins[i],
                        self.placement.rotations[i],
                    ),
                );
            }
        } else {
            for old in &self.moved {
                if let Some(site) = old.site
                    && self.occupant[site.index()] == old.item.0
                {
                    self.occupant[site.index()] = EMPTY;
                }
            }
            for old in &self.moved {
                if let Some(site) = self.placement.sites[old.item.index()] {
                    self.occupant[site.index()] = old.item.0;
                }
            }
        }
    }

    /// Takes back the move just tried.
    fn revert(&mut self) {
        for old in self.moved.iter().rev() {
            let i = old.item.index();
            self.placement.origins[i] = old.origin;
            self.placement.rotations[i] = old.rotation;
            self.placement.sites[i] = old.site;
        }
    }

    /// A rotation other than the current one, if the item may have one, or `None`.
    fn other_rotation(&mut self, item: ItemId) -> Option<i32> {
        let allowed = &self.problem.item(item).rotations;
        if allowed.len() < 2 || self.rng.below(4) != 0 {
            return None;
        }
        let current = self.placement.rotations[item.index()];
        let r = allowed[self.rng.below(allowed.len() as u64) as usize];
        (r != current).then_some(r)
    }

    fn range_steps(&self) -> i64 {
        self.range as i64
    }

    fn region_allows_site(&self, item: ItemId, site: SiteId) -> bool {
        self.problem.item(item).region.is_none_or(|region| {
            self.problem.regions()[region.index()].contains(self.problem.sites()[site.index()].at)
        })
    }

    /// Whether `item` can be moved by a swap: movable, and not in a macro.
    fn swappable(&self, item: u32) -> bool {
        let unit = self.netlist.unit_of[item as usize];
        unit != NO_UNIT && !self.netlist.units[unit as usize].is_macro
    }

    /// A random site of the kind of the site `from`, within the range limit of it and within
    /// `region`'s columns and rows, other than `from`.
    fn random_site_near(&mut self, from: SiteId, region: Option<Rect>) -> Option<SiteId> {
        let sites = self.problem.sites();
        let kind = sites[from.index()].kind;
        let g = self.netlist.grid_of(kind)?;
        let grid = &self.netlist.grids[g];
        let (cx, cy, _) = self.netlist.site_cell[from.index()];
        let r = self.range_steps();
        let (mut lo_x, mut hi_x) = (cx as i64 - r, cx as i64 + r);
        let (mut lo_y, mut hi_y) = (cy as i64 - r, cy as i64 + r);
        lo_x = lo_x.max(0);
        lo_y = lo_y.max(0);
        hi_x = hi_x.min(grid.xs.len() as i64 - 1);
        hi_y = hi_y.min(grid.ys.len() as i64 - 1);
        if let Some(region) = region {
            lo_x = lo_x.max(grid.xs.partition_point(|&x| x < region.min.x) as i64);
            hi_x = hi_x.min(grid.xs.partition_point(|&x| x <= region.max.x) as i64 - 1);
            lo_y = lo_y.max(grid.ys.partition_point(|&y| y < region.min.y) as i64);
            hi_y = hi_y.min(grid.ys.partition_point(|&y| y <= region.max.y) as i64 - 1);
        }
        if lo_x > hi_x || lo_y > hi_y {
            return None;
        }
        for _ in 0..8 {
            let x = lo_x + self.rng.below((hi_x - lo_x + 1) as u64) as i64;
            let y = lo_y + self.rng.below((hi_y - lo_y + 1) as u64) as i64;
            let cell = grid.cell(x as usize, y as usize);
            if cell.is_empty() {
                continue;
            }
            let site = cell[self.rng.below(cell.len() as u64) as usize];
            if site != from {
                return Some(site);
            }
        }
        None
    }

    fn propose_on_sites(&mut self, unit: usize) -> bool {
        let members = self.netlist.units[unit].members.len();
        if !self.netlist.units[unit].is_macro {
            let item = self.netlist.units[unit].members[0].0;
            let i = item.index();
            let Some(from) = self.placement.sites[i] else {
                return false;
            };
            if let Some(rotation) = self.other_rotation(item) {
                self.proposal
                    .push((item, self.placement.origins[i], rotation, Some(from)));
                return true;
            }
            let region = self
                .problem
                .item(item)
                .region
                .map(|r| self.problem.regions()[r.index()]);
            let Some(to) = self.random_site_near(from, region) else {
                return false;
            };
            let sites = self.problem.sites();
            let rotation = self.placement.rotations[i];
            let other = self.occupant[to.index()];
            if other != EMPTY {
                if !self.swappable(other) || !self.region_allows_site(ItemId(other), from) {
                    return false;
                }
                let o = other as usize;
                self.proposal.push((
                    ItemId(other),
                    sites[from.index()].at,
                    self.placement.rotations[o],
                    Some(from),
                ));
            }
            self.proposal
                .push((item, sites[to.index()].at, rotation, Some(to)));
            return true;
        }

        // A macro: shift every member by the same displacement.
        let (first, _) = self.netlist.units[unit].members[0];
        let Some(from) = self.placement.sites[first.index()] else {
            return false;
        };
        let Some(to) = self.random_site_near(from, None) else {
            return false;
        };
        let sites = self.problem.sites();
        let shift = sites[to.index()].at.minus(sites[from.index()].at);
        let mut targets = Vec::with_capacity(members);
        let mut sources = Vec::with_capacity(members);
        for m in 0..members {
            let (item, _) = self.netlist.units[unit].members[m];
            let Some(source) = self.placement.sites[item.index()] else {
                return false;
            };
            let kind = self.problem.item(item).kind;
            let Some(g) = self.netlist.grid_of(kind) else {
                return false;
            };
            let grid = &self.netlist.grids[g];
            let Some((x, y)) = grid.locate(sites[source.index()].at.offset(shift)) else {
                return false;
            };
            let cell = grid.cell(x, y);
            let sub = self.netlist.site_cell[source.index()].2 as usize;
            let Some(&target) = cell.get(sub) else {
                return false;
            };
            if !self.region_allows_site(item, target) {
                return false;
            }
            sources.push(source);
            targets.push(target);
        }
        // Onto another macro of the same shape, exactly: the two trade places.
        let first_other = self.occupant[targets[0].index()];
        if first_other != EMPTY {
            let other_unit = self.netlist.unit_of[first_other as usize];
            if other_unit != NO_UNIT
                && other_unit != unit as u32
                && self.netlist.units[other_unit as usize].is_macro
            {
                let theirs = &self.netlist.units[other_unit as usize].members;
                let same = theirs.len() == members
                    && (0..members).all(|m| {
                        let (mine, _) = self.netlist.units[unit].members[m];
                        let (their, _) = theirs[m];
                        self.occupant[targets[m].index()] == their.0
                            && self.problem.item(mine).kind == self.problem.item(their).kind
                            && self.region_allows_site(their, sources[m])
                    });
                if !same {
                    return false;
                }
                for m in 0..members {
                    let (mine, _) = self.netlist.units[unit].members[m];
                    let (their, _) = self.netlist.units[other_unit as usize].members[m];
                    self.proposal.push((
                        their,
                        sites[sources[m].index()].at,
                        self.placement.rotations[their.index()],
                        Some(sources[m]),
                    ));
                    self.proposal.push((
                        mine,
                        sites[targets[m].index()].at,
                        self.placement.rotations[mine.index()],
                        Some(targets[m]),
                    ));
                }
                return true;
            }
        }
        for m in 0..members {
            let (item, _) = self.netlist.units[unit].members[m];
            let target = targets[m];
            let other = self.occupant[target.index()];
            if other != EMPTY && self.netlist.unit_of[other as usize] != unit as u32 {
                if !self.swappable(other) {
                    return false;
                }
                // Follow the shift back to a site that no member takes.
                let mut free = sources[m];
                let mut guard = 0;
                while let Some(k) = targets.iter().position(|&t| t == free) {
                    free = sources[k];
                    guard += 1;
                    if guard > members {
                        return false;
                    }
                }
                if sites[free.index()].kind != self.problem.item(ItemId(other)).kind
                    || !self.region_allows_site(ItemId(other), free)
                {
                    return false;
                }
                self.proposal.push((
                    ItemId(other),
                    sites[free.index()].at,
                    self.placement.rotations[other as usize],
                    Some(free),
                ));
            }
            self.proposal.push((
                item,
                sites[target.index()].at,
                self.placement.rotations[item.index()],
                Some(target),
            ));
        }
        true
    }

    fn propose_in_free_space(&mut self, unit: usize) -> bool {
        let r = self.range_steps();
        let (area, pitch) = (self.area, self.pitch);
        let is_macro = self.netlist.units[unit].is_macro;
        let (first, first_offset) = self.netlist.units[unit].members[0];
        let anchor = self.placement.origins[first.index()].minus(first_offset);
        let mut rotation = self.placement.rotations[first.index()];
        let mut target = anchor;
        let turned = if is_macro {
            None
        } else {
            self.other_rotation(first)
        };
        if let Some(turn) = turned {
            rotation = turn;
        } else {
            let dx = self.rng.below((2 * r + 1) as u64) as i64 - r;
            let dy = self.rng.below((2 * r + 1) as u64) as i64 - r;
            target = Point::new(anchor.x + dx * pitch, anchor.y + dy * pitch);
        }
        let unit_ref = &self.netlist.units[unit];
        let bounds = unit_box(self.problem, unit_ref, rotation);
        let lowest = Point::new(area.min.x - bounds.min.x, area.min.y - bounds.min.y);
        let highest = Point::new(area.max.x - bounds.max.x, area.max.y - bounds.max.y);
        if highest.x < lowest.x || highest.y < lowest.y {
            return false;
        }
        let target = snap(
            Rect {
                min: lowest,
                max: highest,
            }
            .clamp(target),
            &bounds,
            &area,
            pitch,
        );
        let target = Rect {
            min: lowest,
            max: highest,
        }
        .clamp(target);
        if target == anchor && turned.is_none() {
            return false;
        }
        let Some(index) = &self.index else {
            return false;
        };

        // A single item onto another single item swaps with it, centre for centre.
        if !is_macro && turned.is_none() {
            let new_outline = self.problem.outline_at(first, target, rotation);
            let centre = centre(&new_outline);
            if let Some(other) = index.item_at(centre)
                && other != first.0
                && self.swappable(other)
            {
                let o = ItemId(other);
                let other_rotation = self.placement.rotations[o.index()];
                let other_outline =
                    self.problem
                        .outline_at(o, self.placement.origins[o.index()], other_rotation);
                let old_outline = self.problem.outline_at(first, anchor, rotation);
                // Put each where the other's centre was.
                let mine = anchor.offset(centre_of(&other_outline).minus(centre_of(&old_outline)));
                let theirs = self.placement.origins[o.index()]
                    .offset(centre_of(&old_outline).minus(centre_of(&other_outline)));
                let my_bounds = bounds;
                let their_bounds = unit_box(
                    self.problem,
                    &self.netlist.units[self.netlist.unit_of[o.index()] as usize],
                    other_rotation,
                );
                let mine = snap(mine, &my_bounds, &area, pitch);
                let theirs = snap(theirs, &their_bounds, &area, pitch);
                let skip = |i: u32| i == first.0 || i == other;
                let my_outline = self.problem.outline_at(first, mine, rotation);
                let their_outline = self.problem.outline_at(o, theirs, other_rotation);
                let fits = |item: ItemId, outline: &Rect| {
                    area.contains_rect(outline)
                        && self.problem.item(item).region.is_none_or(|region| {
                            self.problem.regions()[region.index()].contains_rect(outline)
                        })
                        && !index.overlaps(outline, skip)
                };
                if fits(first, &my_outline)
                    && fits(o, &their_outline)
                    && !my_outline.overlaps(&their_outline)
                {
                    self.proposal.push((first, mine, rotation, None));
                    self.proposal.push((o, theirs, other_rotation, None));
                    return true;
                }
                return false;
            }
        }

        if !unit_fits(self.problem, unit_ref, rotation, target, &area, index) {
            return false;
        }
        for &(item, offset) in &unit_ref.members {
            let r = if is_macro {
                self.placement.rotations[item.index()]
            } else {
                rotation
            };
            self.proposal.push((item, target.offset(offset), r, None));
        }
        true
    }
}

fn centre(rect: &Rect) -> Point {
    centre_of(rect)
}

fn centre_of(rect: &Rect) -> Point {
    Point::new(
        rect.min.x + (rect.max.x - rect.min.x) / 2,
        rect.min.y + (rect.max.y - rect.min.y) / 2,
    )
}

/// The cube root of a non-negative value, by Newton's method from basic operations only, so
/// that it is the same everywhere.
fn cube_root(v: f64) -> f64 {
    if v <= 0.0 {
        return 0.0;
    }
    let mut x = if v > 1.0 { v / 3.0 } else { 1.0 };
    for _ in 0..200 {
        let next = x - (x * x * x - v) / (3.0 * x * x);
        if next == x {
            break;
        }
        x = next;
    }
    x
}

/// Anneals from a random legal placement, made by legalizing random targets.
///
/// # Errors
///
/// [`PlaceError::Problem`] if the problem is malformed, and [`PlaceError::NoRoom`] if the
/// random start can't be made legal.
pub fn anneal<C: CostTerm>(
    problem: &PlacementProblem,
    cost: C,
    options: AnnealOptions<'_>,
) -> Result<PlaceOutcome, PlaceError> {
    problem.validate()?;
    let netlist = Netlist::new(problem);
    let mut rng = Rng::new(options.seed ^ 0x9E37_79B9_7F4A_7C15);
    let extent = problem.extent();
    let anchors: Vec<Point> = netlist
        .units
        .iter()
        .map(|_| {
            Point::new(
                extent.min.x + rng.below(extent.width() + 1) as i64,
                extent.min.y + rng.below(extent.height() + 1) as i64,
            )
        })
        .collect();
    let rotations: Vec<i32> = netlist
        .units
        .iter()
        .map(|unit| problem.item(unit.members[0].0).rotations[0])
        .collect();
    let start = legalize_units(problem, &netlist, &anchors, &rotations)?;
    Ok(Annealer::with_netlist(problem, cost, start, options, netlist).run())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_roots() {
        assert_eq!(cube_root(0.0), 0.0);
        assert!((cube_root(27.0) - 3.0).abs() < 1e-12);
        assert!((cube_root(1000.0) - 10.0).abs() < 1e-12);
        assert!((cube_root(0.125) - 0.5).abs() < 1e-12);
    }
}
