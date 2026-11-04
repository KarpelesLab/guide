//! Multi-net routing engines.
//!
//! [`Engine`] is a sequential negotiated-congestion router, after PathFinder (McMurchie and
//! Ebeling, 1995). Nets may share resources at first, at a price, and the price rises until
//! they stop. [`route`] runs one to the end, and stepping an [`Engine`] yourself shows the
//! negotiation one iteration at a time.
//!
//! # The negotiation
//!
//! Each iteration rips up nets and routes them again, one at a time, each seeing what the
//! others hold. A step into resource `r` costs
//!
//! ```text
//! (base + history(r)) * (1 + present * overuse(r))
//! ```
//!
//! where `base` is the step's cost in the space and `overuse(r)` is how far beyond its
//! capacity `r` would be with this net added. `present` grows each iteration up to a cap, so
//! that sharing gets ever less attractive, and `history(r)` grows each iteration by
//! `history_factor` times the overuse of `r`, so that a resource that keeps being fought
//! over stays dear even at the moments when it is free. That is what stops two nets from
//! taking turns on the same resource for ever. A step that consumes several resources adds
//! up their histories and their overuses. See [`EngineOptions`] for the schedule.
//!
//! The formula is additive in history, unlike the multiplicative form that some routers
//! use, because a zero-cost step has to be able to get dearer: multiplying a base of zero
//! gives zero, and nets would share free resources for ever. A consequence is that
//! `history_factor` is in the units of the space's costs.
//!
//! The first iteration routes every net. After that, only nets that hold an overused
//! resource at the moment their turn comes up are routed again, unless
//! [`reroute_all`](EngineOptions::reroute_all) is set. A net whose conflict was settled by
//! an earlier net that same iteration is left alone.
//!
//! Prices only ever raise a step's cost, so a space's lower bound stays admissible, and they
//! take no step away, so whether a path exists never depends on them.
//!
//! # Multi-terminal nets
//!
//! A net's tree grows from its driver, the first terminal. The first search starts from the
//! driver's access points, at their access costs plus whatever occupying them is priced at,
//! and every later one from all the nodes of the tree at no cost. Each search looks for the
//! nearest access point of any terminal not yet connected. An access point's cost is charged
//! at the end of the search, through [`Target`], so a cheap access point further away can
//! beat a dear one nearby, and the access cost plays its part in choosing a route rather
//! than only being reported. A terminal is connected as soon as the tree touches any of
//! its access points, even when a path to another terminal happens to pass through.
//!
//! The choice of root can be wrong in a directed space: a driver's access point might reach
//! the nearest terminal but not the next. So a net whose tree gets stuck starts again without
//! that root, which it remembers, as prices cannot make it viable later. A net is reported
//! as [`NoPath`](UnroutedReason::NoPath) only when no access point of its driver reaches all
//! of its terminals, and is not searched again after that.
//!
//! Nets with fewer than two terminals get an empty route, which occupies nothing.
//!
//! # Occupancy
//!
//! A net counts once per resource, however many of its edges consume it, and that includes
//! what its root occupies, through a call to
//! [`resources`](RoutingSpace::resources) with the root as both ends. While a tree grows,
//! resources it already holds are charged at nothing but the step's base cost, since using
//! them again takes nothing more from anyone.
//!
//! # Budgets and cancellation
//!
//! A run ends at [`max_iterations`](EngineOptions::max_iterations), at
//! [`max_expansions`](EngineOptions::max_expansions), or when the
//! [`cancel`](EngineOptions::cancel) flag is set. The flag is read before every net and
//! during long searches. A net interrupted part way through keeps the route it had.
//!
//! There is deliberately no time limit. How far a run gets in a given time depends on the
//! machine and its load, so a time limit would make the result depend on them too. An
//! expansion budget stops at the same point everywhere. A caller that does need a deadline
//! can set the cancel flag from a timer, knowingly giving up reproducibility for that run.
//!
//! # Results
//!
//! [`Outcome`] holds the best iteration seen, chosen by fewest unrouted nets, then least
//! total overuse, then least total cost, with the earliest winning ties. A run that ends
//! with conflicts **keeps the conflicting routes**, and [`Outcome::overused`] lists every
//! overused resource with the nets on it. [`Outcome::is_success`] is true only if every net
//! is routed and nothing is overused, and [`Outcome::legal_routes`] returns the routes only
//! in that case.
//!
//! The alternative was to unroute the nets in conflict. That would discard information a
//! caller can use, such as where the congestion is and which nets are involved, and it would
//! have to pick which of the nets on a resource to sacrifice. It would also need a reason
//! that [`UnroutedReason`] does not have, and calling congestion a spent budget or a missing
//! path would be untrue.
//!
//! # Determinism
//!
//! The same space, nets and options give the same [`Outcome`], with any platform's floating
//! point, as long as nothing sets the cancel flag at an unpredictable moment. Nets go in the
//! order they are given, or a seeded shuffle of it. Hash tables here are looked up and never
//! iterated, resources are put in order before their state is touched, and totals are
//! summed over nets in their given order.
//!
//! # State for parallel routing
//!
//! Routing a net reads the resource table and writes only to its own worker and to its
//! record of useless roots. Changes to the table happen apart from that, when the route is
//! committed. So a batch of nets that cannot interact could be routed with a worker each and
//! then committed in a fixed order, to the same result. All of this is sequential for now.

use core::cell::RefCell;
use core::fmt;
use core::hash::{BuildHasherDefault, Hash, Hasher};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cost::Cost;
use crate::id::NetId;
use crate::net::{Net, Route, RouteSet, RouteStats, UnroutedReason};
use crate::rng::Rng;
use crate::search::{Search, SearchLimits, SearchOutcome, Source, Target};
use crate::space::{RoutingSpace, Step};

/// What an [`Engine`] may do, and when it has to stop.
///
/// Create this with [`Default`] and set the fields. The costs are [`Cost`]s so that they
/// cannot be negative or NaN.
///
/// The factors start from the values of VPR and reticle, whose routers have seen far harder
/// problems than this crate's tests, and differ only where the docs of a field say so.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EngineOptions<'c> {
    /// How many iterations to run at most, which is what ends a run on a problem that cannot
    /// be solved. The default is 100.
    ///
    /// VPR and reticle allow 50 and 40, but with few nets left to route late in a run, more
    /// iterations cost little when they are not needed. On dense grids, 100 instead of 50
    /// took the share of problems solved from 58 of 72 to 71 of 72, for 3% more expansions.
    /// The price is paid on a problem that cannot be solved, which takes twice as long to
    /// give up on.
    pub max_iterations: u32,
    /// How many nodes all searches together may expand. The default is no limit.
    pub max_expansions: u64,
    /// A flag that stops the run once set, from any thread. The default is none.
    pub cancel: Option<&'c AtomicBool>,
    /// The present factor of the first iteration. The default is 0.5, the same as the second.
    ///
    /// VPR and reticle use 0 here, so that each net first takes its own shortest path. reticle
    /// measured searches flooding on a real FPGA without it, because no lower bound knows
    /// what earlier nets have taken. That did not happen on the dense grids and small
    /// island-style fabrics measured here, whose lower bounds are close to exact. Against 0,
    /// a charge from the start took 30% to 65% of the expansions, solved at least as many
    /// problems, and gave routes about 4% cheaper, because nets avoid conflicts that they
    /// would otherwise create and then have to resolve. It is worth measuring again on real
    /// fabrics, where 0 may turn out to be the better setting.
    pub first_present: Cost,
    /// The present factor of the second iteration, from which it grows. The default is 0.5,
    /// as in VPR and reticle.
    pub initial_present: Cost,
    /// What the present factor is multiplied by after each iteration from the second. The
    /// default is 1.8, as in reticle, where VPR uses 1.3. On dense grids, 1.3 took about 1.5
    /// times the expansions and solved fewer problems within the same number of iterations.
    pub present_growth: Cost,
    /// The most the present factor can be. The default is 1000, as in VPR.
    ///
    /// At this level, the overuse term already outweighs anything a net could save by
    /// sharing. Above it, costs lose precision for no change in behaviour.
    pub max_present: Cost,
    /// How much history one unit of overuse adds per iteration. The default is 1, as in VPR
    /// and reticle, which suits a space whose typical step costs about 1. Scale it with the
    /// step costs of other spaces, because history adds to them. On dense grids, both 0.2
    /// and 5 did worse. At zero, a step that costs nothing can never become dear.
    pub history_factor: Cost,
    /// Whether to route every net in every iteration, and not only those holding an overused
    /// resource. The default is false. On dense grids this rescued one problem out of 72 that
    /// was otherwise stuck, and its routes were no better, for about twice the expansions.
    pub reroute_all: bool,
    /// A seed to shuffle the order of the nets with before each iteration after the first.
    /// The default is none, which keeps the given order throughout.
    ///
    /// This did not help on average, so it is off. On dense grids and fabrics it left
    /// iterations and expansions about the same or a little worse, and routes up to 1% dearer.
    /// It is here as a way out, should a problem get stuck in a cycle: it rescued one
    /// problem out of 72.
    pub shuffle_seed: Option<u64>,
}

impl Default for EngineOptions<'_> {
    fn default() -> Self {
        EngineOptions {
            max_iterations: 100,
            max_expansions: u64::MAX,
            cancel: None,
            first_present: Cost::new(0.5).unwrap_or(Cost::ONE),
            initial_present: Cost::new(0.5).unwrap_or(Cost::ONE),
            present_growth: Cost::new(1.8).unwrap_or(Cost::ONE),
            max_present: Cost::from(1000),
            history_factor: Cost::ONE,
            reroute_all: false,
            shuffle_seed: None,
        }
    }
}

/// Why an [`Engine`] could not be created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EngineError {
    /// Two nets have the same ID, so their results could not be told apart.
    DuplicateNet {
        /// The repeated ID.
        net: NetId,
    },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::DuplicateNet { net } => write!(f, "net {} is listed twice", net.0),
        }
    }
}

impl Error for EngineError {}

/// Why an [`Engine`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StopReason {
    /// Nothing is overused, so there is nothing left to negotiate. Nets with no path may
    /// still be unrouted: see [`Outcome::is_success`].
    Converged,
    /// [`max_iterations`](EngineOptions::max_iterations) ran out with conflicts left.
    IterationLimit,
    /// [`max_expansions`](EngineOptions::max_expansions) ran out.
    BudgetExhausted,
    /// The [`cancel`](EngineOptions::cancel) flag was set.
    Cancelled,
    /// The engine has not stopped: the caller asked for its result before it finished.
    NotFinished,
}

/// What one iteration of an [`Engine`] did, and the state it left behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct IterationReport {
    /// The number of this iteration, from 1.
    pub iteration: u32,
    /// The present factor used.
    pub present: Cost,
    /// Whether the iteration got through all of its nets, as opposed to being interrupted.
    pub completed: bool,
    /// How many nets were routed again.
    pub nets_rerouted: u32,
    /// How many searches ran.
    pub searches: u64,
    /// How many nodes those searches expanded.
    pub nodes_expanded: u64,
    /// How many nets that need a route have none.
    pub unrouted: u32,
    /// How many resources are overused.
    pub overused_resources: u32,
    /// The sum over resources of how far each is beyond its capacity.
    pub total_overuse: u64,
    /// The total cost of all routes, as defined at [`Outcome::total_cost`].
    pub total_cost: Cost,
}

/// A resource with more nets on it than its capacity allows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct OverusedResource<R> {
    /// The resource.
    pub resource: R,
    /// How many nets hold it, which is more than `capacity`.
    pub occupancy: u32,
    /// How many nets it can take.
    pub capacity: u32,
    /// The nets that hold it, in ascending order.
    pub nets: Vec<NetId>,
}

/// The result of a run: the best iteration seen, and the plain truth about it.
///
/// **Check [`is_success`](Outcome::is_success) before trusting the routes.** A run that stops
/// with conflicts keeps its routes, so [`routes`](Outcome::routes) can look complete while
/// [`overused`](Outcome::overused) says that they are not legal. See the
/// [module documentation](self) for why.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[must_use = "a result may hold conflicts, which only checking it will show"]
pub struct Outcome<N, R> {
    /// A route or a reason for every net. Its statistics cover the whole run, not only the
    /// iteration these routes come from.
    pub routes: RouteSet<N>,
    /// Every resource that is overused by these routes, in ascending order.
    pub overused: Vec<OverusedResource<R>>,
    /// Why the engine stopped.
    pub stop: StopReason,
    /// The iteration these routes come from, from 1, or 0 if no iteration ran.
    pub best_iteration: u32,
    /// The total base cost of these routes. For each routed net in turn, this is the access
    /// cost of its root, then for each path the costs of the new steps in the space, before
    /// any pricing, and the lowest access cost among the access points through which that
    /// path connected each terminal.
    pub total_cost: Cost,
}

impl<N, R> Outcome<N, R> {
    /// Returns whether every net is routed and no resource is overused.
    pub fn is_success(&self) -> bool {
        self.overused.is_empty() && self.routes.unrouted().next().is_none()
    }

    /// Returns the routes if they are all there and legal, and otherwise `None`.
    pub fn legal_routes(&self) -> Option<&RouteSet<N>> {
        self.is_success().then_some(&self.routes)
    }

    /// Returns the nets involved in a conflict, in ascending order.
    pub fn conflicted_nets(&self) -> Vec<NetId> {
        let mut nets: Vec<NetId> = self
            .overused
            .iter()
            .flat_map(|entry| entry.nets.iter().copied())
            .collect();
        nets.sort_unstable();
        nets.dedup();
        nets
    }
}

/// Routes `nets` through `space` to the end, under `options`.
///
/// This is [`Engine::new`] followed by [`Engine::run`].
///
/// # Errors
///
/// Returns an error if two nets have the same ID.
///
/// # Examples
///
/// Two nets that want to cross on a board of two layers. Each takes its straight line at
/// first, which conflicts where they meet, and the negotiation settles it with a via.
///
/// ```
/// use guide::engine::{EngineOptions, StopReason, route};
/// use guide::id::NetId;
/// use guide::net::{Net, Terminal};
/// use guide::space::grid::Grid;
///
/// let grid = Grid::new(5, 5, 2)?;
/// let at = |x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
/// let nets = [
///     Net { id: NetId(0), terminals: vec![at(0, 2), at(4, 2)] },
///     Net { id: NetId(1), terminals: vec![at(2, 0), at(2, 4)] },
/// ];
///
/// let outcome = route(&grid, &nets, EngineOptions::default()).unwrap();
///
/// assert_eq!(outcome.stop, StopReason::Converged);
/// let routes = outcome.legal_routes().expect("this problem can be routed");
/// assert_eq!(routes.routes().count(), 2);
/// assert!(routes.stats.iterations >= 2);
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
pub fn route<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
) -> Result<Outcome<S::Node, S::Resource>, EngineError> {
    Ok(Engine::new(space, nets, options)?.run())
}

/// A map for hot lookups, keyed by small plain values and never iterated.
type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
/// A set for hot lookups, keyed by small plain values and never iterated.
type FastSet<K> = HashSet<K, BuildHasherDefault<FastHasher>>;

/// A multiplicative hash, in the style of FxHash, for the lookups made once per search step.
///
/// The keys are node and resource identifiers chosen by the problem, and an adversary who
/// could choose them could only slow a run down, so std's collision-resistant hash is not
/// worth what it costs here. No result depends on the hash values, because these
/// tables are never iterated.
#[derive(Clone, Copy, Default)]
struct FastHasher(u64);

impl FastHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        const MULTIPLIER: u64 = 0xF135_7AEA_2E62_A9C5;
        self.0 = (self.0 ^ word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u16(&mut self, value: u16) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        // A multiplication leaves its best bits at the top and the hash table reads the
        // bottom ones for the bucket.
        self.0.rotate_left(26)
    }
}

/// The negotiation state of one resource.
#[derive(Clone, Copy)]
struct ResourceState {
    /// How many nets hold the resource.
    occupancy: u32,
    /// Copied here because a space's answer could mean a cache miss on every step.
    capacity: u32,
    history: Cost,
}

impl ResourceState {
    /// How far beyond capacity the resource is.
    #[inline]
    fn overuse(self) -> u32 {
        self.occupancy.saturating_sub(self.capacity)
    }
}

/// How far beyond capacity a resource would be with one more net on it.
#[inline]
fn overuse_with_one_more(occupancy: u32, capacity: u32) -> u32 {
    occupancy.saturating_add(1).saturating_sub(capacity)
}

/// Every resource a net has ever held.
///
/// Spaces can have far more resources than a problem touches, and there is no way to ask
/// a space for a dense index, so entries are made when a route first commits to a resource.
/// A resource with no entry is free and has no history, which is the common case and takes
/// one failed lookup to find out.
struct ResourceTable<R> {
    /// Lookups only, as iterating it would break determinism. `states` is what is iterated.
    index: FastMap<R, u32>,
    resources: Vec<R>,
    states: Vec<ResourceState>,
}

impl<R: Copy + Eq + Ord + Hash> ResourceTable<R> {
    fn new() -> Self {
        ResourceTable {
            index: FastMap::default(),
            resources: Vec::new(),
            states: Vec::new(),
        }
    }

    /// Returns the entry for `resource`, making it if needed.
    fn intern<S: RoutingSpace<Resource = R>>(&mut self, space: &S, resource: R) -> u32 {
        *self.index.entry(resource).or_insert_with(|| {
            self.resources.push(resource);
            self.states.push(ResourceState {
                occupancy: 0,
                capacity: space.capacity(resource),
                history: Cost::ZERO,
            });
            (self.states.len() - 1) as u32
        })
    }

    /// Adds a net to each of `entries`.
    fn acquire(&mut self, entries: &[u32]) {
        for &entry in entries {
            let state = &mut self.states[entry as usize];
            state.occupancy = state.occupancy.saturating_add(1);
        }
    }

    /// Removes a net from each of `entries`.
    fn release(&mut self, entries: &[u32]) {
        for &entry in entries {
            let state = &mut self.states[entry as usize];
            state.occupancy = state.occupancy.saturating_sub(1);
        }
    }

    /// Returns how many resources are overused, and by how much in total.
    fn overuse(&self) -> (u32, u64) {
        let mut resources = 0;
        let mut total = 0;
        for state in &self.states {
            let overuse = state.overuse();
            if overuse > 0 {
                resources += 1;
                total += u64::from(overuse);
            }
        }
        (resources, total)
    }

    /// Charges every overused resource for the iteration just finished.
    fn add_history(&mut self, factor: Cost) {
        for state in &mut self.states {
            let overuse = state.overuse();
            if overuse > 0 {
                state.history = state.history + factor * Cost::from(overuse);
            }
        }
    }
}

/// A space whose steps cost what the negotiation currently says they do.
///
/// Everything else goes straight to the space inside. This exists for one search at a time,
/// borrowing the table, so it is created cheaply and only ever reads shared state.
struct Priced<'a, S: RoutingSpace> {
    inner: &'a S,
    table: &'a ResourceTable<S::Resource>,
    present: Cost,
    /// What the current net already holds. Using these again takes nothing from anyone.
    held: &'a FastSet<S::Resource>,
    /// A buffer for the space's answers. It is here because expansion only gets `&self`, and
    /// it is only ever borrowed within a single call.
    buffer: &'a RefCell<Vec<S::Resource>>,
}

impl<S: RoutingSpace> Priced<'_, S> {
    /// Applies the module's price formula to a step that consumes `resources`.
    #[inline]
    fn price(&self, base: Cost, resources: &mut Vec<S::Resource>) -> Cost {
        // The sums below must not count a resource twice or depend on the order a space
        // lists resources in. Nearly every step has one, so this is rarely any work.
        if resources.len() > 1 {
            resources.sort_unstable();
            resources.dedup();
        }

        let mut history = Cost::ZERO;
        let mut overuse = 0u32;
        for &resource in resources.iter() {
            if !self.held.is_empty() && self.held.contains(&resource) {
                continue;
            }
            let here = match self.table.index.get(&resource) {
                Some(&entry) => {
                    let state = self.table.states[entry as usize];
                    history = history + state.history;
                    overuse_with_one_more(state.occupancy, state.capacity)
                }
                // Free, and yet it can still be overused by a first net if its capacity is
                // zero, and a search must see that up front. Otherwise each iteration would
                // discover only those it blundered into, and a problem with many would run
                // out of iterations.
                None => overuse_with_one_more(0, self.inner.capacity(resource)),
            };
            overuse = overuse.saturating_add(here);
        }

        if history == Cost::ZERO && overuse == 0 {
            return base;
        }
        (base + history) * (Cost::ONE + self.present * Cost::from(overuse))
    }

    /// Returns what it costs to start a tree at `node`, given its access cost.
    fn source_cost(&self, net: NetId, node: S::Node, access: Cost) -> Cost {
        let mut buffer = self.buffer.borrow_mut();
        buffer.clear();
        self.inner.resources(net, node, node, &mut buffer);
        self.price(access, &mut buffer)
    }
}

impl<S: RoutingSpace> RoutingSpace for Priced<'_, S> {
    type Node = S::Node;
    type Resource = S::Resource;
    type Slots = S::Slots;

    fn new_slots(&self) -> S::Slots {
        self.inner.new_slots()
    }

    #[inline]
    fn expand(&self, net: NetId, from: S::Node, out: &mut Vec<Step<S::Node>>) {
        let start = out.len();
        self.inner.expand(net, from, out);
        let mut buffer = self.buffer.borrow_mut();
        for step in &mut out[start..] {
            buffer.clear();
            self.inner.resources(net, from, step.to, &mut buffer);
            step.cost = self.price(step.cost, &mut buffer);
        }
    }

    #[inline]
    fn lower_bound(&self, net: NetId, from: S::Node, to: S::Node) -> Cost {
        self.inner.lower_bound(net, from, to)
    }

    fn resources(&self, net: NetId, from: S::Node, to: S::Node, out: &mut Vec<S::Resource>) {
        self.inner.resources(net, from, to, out);
    }

    fn capacity(&self, resource: S::Resource) -> u32 {
        self.inner.capacity(resource)
    }
}

/// Why a net's routing stopped early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interrupt {
    Budget,
    Cancel,
}

impl Interrupt {
    fn reason(self) -> UnroutedReason {
        match self {
            Interrupt::Budget => UnroutedReason::BudgetExhausted,
            Interrupt::Cancel => UnroutedReason::Cancelled,
        }
    }

    fn stop(self) -> StopReason {
        match self {
            Interrupt::Budget => StopReason::BudgetExhausted,
            Interrupt::Cancel => StopReason::Cancelled,
        }
    }
}

/// The result of routing one net, not yet committed to the resource table.
enum Attempt<N, R> {
    Routed {
        route: Route<N>,
        cost: Cost,
        /// Everything the route consumes, in ascending order and without repeats.
        resources: Vec<R>,
    },
    NoPath,
    Interrupted(Interrupt),
}

/// Everything one thread needs in order to route nets. Nothing here outlives a net except
/// allocations and counters.
struct Worker<S: RoutingSpace> {
    search: Search<S>,
    buffer: RefCell<Vec<S::Resource>>,
    /// What the tree being grown holds, for quick membership tests.
    held: FastSet<S::Resource>,
    /// The same in the order first held, because the set must not be iterated.
    held_list: Vec<S::Resource>,
    sources: Vec<Source<S::Node>>,
    targets: Vec<Target<S::Node>>,
    /// Indices of the terminals still to connect.
    pending: Vec<usize>,
    path: Vec<S::Node>,
    /// The nodes new to the tree after a path, for finding the terminals it connected.
    new_nodes: FastSet<S::Node>,
    steps: Vec<Step<S::Node>>,
    searches: u64,
    expanded: u64,
}

impl<S: RoutingSpace> Worker<S> {
    fn new(space: &S) -> Self {
        Worker {
            search: Search::new(space),
            buffer: RefCell::new(Vec::new()),
            held: FastSet::default(),
            held_list: Vec::new(),
            sources: Vec::new(),
            targets: Vec::new(),
            pending: Vec::new(),
            path: Vec::new(),
            new_nodes: FastSet::default(),
            steps: Vec::new(),
            searches: 0,
            expanded: 0,
        }
    }

    /// Records what `net` consumes by stepping from `from` to `to`.
    fn hold(&mut self, space: &S, net: NetId, from: S::Node, to: S::Node) {
        let buffer = self.buffer.get_mut();
        buffer.clear();
        space.resources(net, from, to, buffer);
        for &resource in buffer.iter() {
            if self.held.insert(resource) {
                self.held_list.push(resource);
            }
        }
    }

    /// Adds the edges from the most recent path to the tree, and returns what they added to
    /// its cost and how far along the path they start.
    fn adopt_path(&mut self, space: &S, net: NetId, route: &mut Route<S::Node>) -> (Cost, usize) {
        let starting = route.is_empty();
        // Sources are the tree itself, so a path only leaves the tree, and has no repeats
        // because each node of it has a single parent. A failure would mean a broken
        // `NodeSlots` that gives two nodes one slot.
        let added = route
            .add_path(&self.path)
            .expect("a search path runs from the tree without repeating a node");
        let from_index = self.path.len() - 1 - added;

        if starting {
            let root = self.path[0];
            self.hold(space, net, root, root);
        }
        let mut cost = Cost::ZERO;
        for index in from_index..self.path.len() - 1 {
            let (from, to) = (self.path[index], self.path[index + 1]);
            self.steps.clear();
            space.expand(net, from, &mut self.steps);
            // Searches do not report the base cost, because they only saw the price. Paths
            // are a tiny part of what a search expands, so asking again costs little.
            if let Some(step) = self.steps.iter().find(|step| step.to == to) {
                cost = cost + step.cost;
            }
            self.hold(space, net, from, to);
        }
        (cost, from_index)
    }

    /// Takes the terminals connected by the most recent path off the list, and returns
    /// the sum of their access costs.
    fn connect_terminals(&mut self, net: &Net<S::Node>, from_index: usize) -> Cost {
        self.new_nodes.clear();
        self.new_nodes.extend(&self.path[from_index..]);
        let new_nodes = &self.new_nodes;
        let mut cost = Cost::ZERO;
        self.pending.retain(|&terminal| {
            let lowest = net.terminals[terminal]
                .access_points()
                .iter()
                .filter(|point| new_nodes.contains(&point.node))
                .map(|point| point.cost)
                .min();
            match lowest {
                Some(access) => {
                    cost = cost + access;
                    false
                }
                None => true,
            }
        });
        cost
    }
}

/// What every net of an iteration sees, which nothing changes while a net is being routed.
struct RouteContext<'a, S: RoutingSpace> {
    space: &'a S,
    table: &'a ResourceTable<S::Resource>,
    present: Cost,
    cancel: Option<&'a AtomicBool>,
}

/// Routes one net that needs it against the current state of the table.
///
/// `budget` is how many expansions it may use, and `dead_roots` lists the driver's access
/// points known not to reach every terminal.
fn route_net<S: RoutingSpace>(
    context: &RouteContext<'_, S>,
    net: &Net<S::Node>,
    budget: u64,
    dead_roots: &mut Vec<S::Node>,
    worker: &mut Worker<S>,
) -> Attempt<S::Node, S::Resource> {
    let space = context.space;
    let mut spent = 0u64;
    let Some(driver) = net.driver() else {
        return Attempt::NoPath;
    };

    // Each time around, one more root is known to be useless, so this ends.
    'roots: loop {
        worker.held.clear();
        worker.held_list.clear();
        worker.pending.clear();
        worker.pending.extend(1..net.terminals.len());
        let mut route = Route::new();
        let mut cost = Cost::ZERO;

        while !worker.pending.is_empty() {
            let priced = Priced {
                inner: space,
                table: context.table,
                present: context.present,
                held: &worker.held,
                buffer: &worker.buffer,
            };

            worker.sources.clear();
            if route.is_empty() {
                for point in driver.access_points() {
                    if !dead_roots.contains(&point.node) {
                        let start = priced.source_cost(net.id, point.node, point.cost);
                        worker.sources.push(Source::with_cost(point.node, start));
                    }
                }
            } else {
                worker.sources.extend(route.nodes().map(Source::new));
            }
            worker.targets.clear();
            for &terminal in &worker.pending {
                let points = net.terminals[terminal].access_points();
                let targets = points
                    .iter()
                    .map(|point| Target::with_cost(point.node, point.cost));
                worker.targets.extend(targets);
            }

            let limits = SearchLimits {
                max_expansions: budget.saturating_sub(spent),
                cancel: context.cancel,
            };
            let outcome = worker.search.find_path_with(
                &priced,
                net.id,
                worker.sources.iter().copied(),
                worker.targets.iter().copied(),
                limits,
            );
            let end = match outcome {
                SearchOutcome::Found(found) => {
                    worker.path.clear();
                    worker.path.extend_from_slice(found.path);
                    Ok(true)
                }
                SearchOutcome::Unreachable => Ok(false),
                SearchOutcome::LimitReached => Err(Interrupt::Budget),
                SearchOutcome::Cancelled => Err(Interrupt::Cancel),
            };
            let expanded = worker.search.stats().expanded;
            spent = spent.saturating_add(expanded);
            worker.searches += 1;
            worker.expanded += expanded;

            match end {
                Ok(true) => {}
                Ok(false) => match route.root() {
                    // With no tree yet, every root that remains has been searched from.
                    None => return Attempt::NoPath,
                    Some(root) => {
                        dead_roots.push(root);
                        continue 'roots;
                    }
                },
                Err(interrupt) => return Attempt::Interrupted(interrupt),
            }

            if route.is_empty() {
                let root = worker.path[0];
                let access = driver
                    .access_points()
                    .iter()
                    .filter(|point| point.node == root)
                    .map(|point| point.cost)
                    .min();
                cost = cost + access.unwrap_or(Cost::ZERO);
            }
            let (steps_cost, from_index) = worker.adopt_path(space, net.id, &mut route);
            cost = cost + steps_cost;
            cost = cost + worker.connect_terminals(net, from_index);
        }

        let mut resources = worker.held_list.clone();
        resources.sort_unstable();
        return Attempt::Routed {
            route,
            cost,
            resources,
        };
    }
}

/// Where one net stands.
struct NetState<N> {
    /// The current route, which is `None` until the net has been routed.
    route: Option<Route<N>>,
    /// The table entries the route holds.
    held: Vec<u32>,
    cost: Cost,
    /// Access points of the driver known not to reach every terminal. Reachability does not
    /// depend on prices, so this is true for the rest of the run.
    dead_roots: Vec<N>,
    no_path: bool,
}

/// The best iteration seen.
struct Best<N, R> {
    routes: RouteSet<N>,
    overused: Vec<OverusedResource<R>>,
    iteration: u32,
    unrouted: u32,
    total_overuse: u64,
    total_cost: Cost,
}

/// A sequential negotiated-congestion router, as a state machine.
///
/// Each call to [`step`](Engine::step) runs one iteration and reports on it. A caller can
/// stop stepping whenever it likes and take the best result so far from
/// [`outcome`](Engine::outcome). See the [module documentation](self) for how it works.
///
/// # Examples
///
/// ```
/// use guide::engine::{Engine, EngineOptions};
/// use guide::id::NetId;
/// use guide::net::{Net, Terminal};
/// use guide::space::grid::Grid;
///
/// let grid = Grid::new(5, 5, 2)?;
/// let at = |x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
/// let nets = [
///     Net { id: NetId(0), terminals: vec![at(0, 2), at(4, 2)] },
///     Net { id: NetId(1), terminals: vec![at(2, 0), at(2, 4)] },
/// ];
///
/// let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
///
/// // Both nets go straight at first, and meet in the middle.
/// let first = engine.step().unwrap();
/// assert_eq!(first.total_overuse, 1);
/// assert_eq!(engine.overused()[0].nets, [NetId(0), NetId(1)]);
/// assert!(!engine.outcome().is_success());
///
/// while let Some(report) = engine.step() {
///     println!("iteration {}: overuse {}", report.iteration, report.total_overuse);
/// }
/// assert!(engine.outcome().is_success());
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
pub struct Engine<'a, S: RoutingSpace> {
    space: &'a S,
    nets: &'a [Net<S::Node>],
    options: EngineOptions<'a>,
    table: ResourceTable<S::Resource>,
    states: Vec<NetState<S::Node>>,
    worker: Worker<S>,
    /// Indices into `nets`, in the order they take their turns.
    order: Vec<u32>,
    rng: Option<Rng>,
    /// The present factor of the most recent iteration.
    present: Cost,
    iteration: u32,
    stats: RouteStats,
    reports: Vec<IterationReport>,
    best: Option<Best<S::Node, S::Resource>>,
    stop: Option<StopReason>,
}

impl<S: RoutingSpace> fmt::Debug for Engine<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("nets", &self.nets.len())
            .field("iteration", &self.iteration)
            .field("stop", &self.stop)
            .finish_non_exhaustive()
    }
}

impl<'a, S: RoutingSpace> Engine<'a, S> {
    /// Creates an engine, which does no work until it is stepped.
    ///
    /// # Errors
    ///
    /// Returns an error if two nets have the same ID.
    pub fn new(
        space: &'a S,
        nets: &'a [Net<S::Node>],
        options: EngineOptions<'a>,
    ) -> Result<Self, EngineError> {
        let mut ids: Vec<NetId> = nets.iter().map(|net| net.id).collect();
        ids.sort_unstable();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(EngineError::DuplicateNet { net: pair[0] });
        }

        let states = nets
            .iter()
            .map(|_| NetState {
                route: None,
                held: Vec::new(),
                cost: Cost::ZERO,
                dead_roots: Vec::new(),
                no_path: false,
            })
            .collect();
        Ok(Engine {
            space,
            nets,
            options,
            table: ResourceTable::new(),
            states,
            worker: Worker::new(space),
            order: (0..nets.len() as u32).collect(),
            rng: options.shuffle_seed.map(Rng::new),
            present: Cost::ZERO,
            iteration: 0,
            stats: RouteStats::default(),
            reports: Vec::new(),
            best: None,
            stop: None,
        })
    }

    /// Runs one iteration and reports on it, or returns `None` if the engine has stopped.
    ///
    /// The iteration that causes the stop is still reported, so the `None` comes from the
    /// call after that one. The exceptions are a cancel flag that is already set between
    /// iterations and an iteration limit of zero, which stop without an iteration to report.
    pub fn step(&mut self) -> Option<IterationReport> {
        if self.stop.is_some() {
            return None;
        }
        if self.cancelled() {
            self.stop = Some(StopReason::Cancelled);
            return None;
        }
        if self.iteration >= self.options.max_iterations {
            self.stop = Some(StopReason::IterationLimit);
            return None;
        }

        self.iteration += 1;
        self.present = self.next_present();
        if self.iteration > 1
            && let Some(rng) = &mut self.rng
        {
            rng.shuffle(&mut self.order);
        }
        let before = self.stats;

        let mut nets_rerouted = 0;
        let mut interrupted = None;
        for position in 0..self.order.len() {
            let index = self.order[position] as usize;
            if !self.wants_routing(index) {
                continue;
            }
            if self.cancelled() {
                interrupted = Some(Interrupt::Cancel);
                break;
            }
            nets_rerouted += 1;
            if let Err(interrupt) = self.reroute(index) {
                interrupted = Some(interrupt);
                break;
            }
        }
        self.stats.iterations += 1;

        let (overused_resources, total_overuse) = self.table.overuse();
        let unrouted = self.unrouted_count();
        // Summed in net order, not turn order, so that shuffling cannot change the rounding
        // of the total for the same set of routes.
        let total_cost: Cost = self
            .states
            .iter()
            .filter(|state| state.route.is_some())
            .map(|state| state.cost)
            .sum();
        self.consider_best(interrupted, unrouted, total_overuse, total_cost);

        let report = IterationReport {
            iteration: self.iteration,
            present: self.present,
            completed: interrupted.is_none(),
            nets_rerouted,
            searches: self.stats.searches - before.searches,
            nodes_expanded: self.stats.nodes_expanded - before.nodes_expanded,
            unrouted,
            overused_resources,
            total_overuse,
            total_cost,
        };
        self.reports.push(report);

        if let Some(interrupt) = interrupted {
            self.stop = Some(interrupt.stop());
        } else if total_overuse == 0 {
            self.stop = Some(StopReason::Converged);
        } else if self.iteration >= self.options.max_iterations {
            self.stop = Some(StopReason::IterationLimit);
        } else {
            self.table.add_history(self.options.history_factor);
        }
        Some(report)
    }

    /// Steps until the engine stops, and returns the result.
    pub fn run(mut self) -> Outcome<S::Node, S::Resource> {
        while self.step().is_some() {}
        self.outcome()
    }

    /// Returns why the engine stopped, or `None` if it can still make progress.
    pub fn stop_reason(&self) -> Option<StopReason> {
        self.stop
    }

    /// Returns the reports of all iterations so far.
    pub fn reports(&self) -> &[IterationReport] {
        &self.reports
    }

    /// Returns the resources overused right now, which need not be by the best iteration.
    pub fn overused(&self) -> Vec<OverusedResource<S::Resource>> {
        self.collect_overused()
    }

    /// Returns the best result so far, which the engine keeps.
    pub fn outcome(&self) -> Outcome<S::Node, S::Resource> {
        let stop = self.stop.unwrap_or(StopReason::NotFinished);
        let (mut routes, overused, best_iteration, total_cost) = match &self.best {
            Some(best) => (
                best.routes.clone(),
                best.overused.clone(),
                best.iteration,
                best.total_cost,
            ),
            None => {
                let reason = match stop {
                    StopReason::IterationLimit | StopReason::BudgetExhausted => {
                        UnroutedReason::BudgetExhausted
                    }
                    _ => UnroutedReason::Cancelled,
                };
                (self.current_routes(reason), Vec::new(), 0, Cost::ZERO)
            }
        };
        routes.stats = self.stats;
        Outcome {
            routes,
            overused,
            stop,
            best_iteration,
            total_cost,
        }
    }

    fn cancelled(&self) -> bool {
        self.options
            .cancel
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Returns the present factor for the iteration about to start.
    fn next_present(&self) -> Cost {
        let options = &self.options;
        let present = match self.iteration {
            0 | 1 => options.first_present,
            2 => options.initial_present,
            _ => self.present * options.present_growth,
        };
        present.min(options.max_present)
    }

    /// Returns whether a net should be routed again now.
    fn wants_routing(&self, index: usize) -> bool {
        let state = &self.states[index];
        if !self.nets[index].needs_routing() || state.no_path {
            return false;
        }
        if state.route.is_none() || self.options.reroute_all {
            return true;
        }
        state
            .held
            .iter()
            .any(|&entry| self.table.states[entry as usize].overuse() > 0)
    }

    /// Rips up a net and routes it again, or puts it back if interrupted.
    fn reroute(&mut self, index: usize) -> Result<(), Interrupt> {
        let net = &self.nets[index];
        let state = &mut self.states[index];
        self.table.release(&state.held);

        let context = RouteContext {
            space: self.space,
            table: &self.table,
            present: self.present,
            cancel: self.options.cancel,
        };
        let budget = self.options.max_expansions;
        let attempt = route_net(
            &context,
            net,
            budget.saturating_sub(self.stats.nodes_expanded),
            &mut state.dead_roots,
            &mut self.worker,
        );
        self.stats.searches += self.worker.searches;
        self.stats.nodes_expanded += self.worker.expanded;
        self.worker.searches = 0;
        self.worker.expanded = 0;

        match attempt {
            Attempt::Routed {
                route,
                cost,
                resources,
            } => {
                state.held.clear();
                for &resource in &resources {
                    state.held.push(self.table.intern(self.space, resource));
                }
                self.table.acquire(&state.held);
                state.route = Some(route);
                state.cost = cost;
                Ok(())
            }
            Attempt::NoPath => {
                state.held.clear();
                state.route = None;
                state.cost = Cost::ZERO;
                state.no_path = true;
                Ok(())
            }
            Attempt::Interrupted(interrupt) => {
                self.table.acquire(&state.held);
                Err(interrupt)
            }
        }
    }

    fn unrouted_count(&self) -> u32 {
        let unrouted = self
            .nets
            .iter()
            .zip(&self.states)
            .filter(|(net, state)| net.needs_routing() && state.route.is_none());
        unrouted.count() as u32
    }

    /// Keeps the current state if it is the best so far.
    fn consider_best(
        &mut self,
        interrupted: Option<Interrupt>,
        unrouted: u32,
        total_overuse: u64,
        total_cost: Cost,
    ) {
        let rank = (unrouted, total_overuse, total_cost);
        let better = self
            .best
            .as_ref()
            .is_none_or(|best| rank < (best.unrouted, best.total_overuse, best.total_cost));
        if better {
            let reason = interrupted.map_or(UnroutedReason::BudgetExhausted, Interrupt::reason);
            self.best = Some(Best {
                routes: self.current_routes(reason),
                overused: self.collect_overused(),
                iteration: self.iteration,
                unrouted,
                total_overuse,
                total_cost,
            });
        }
    }

    /// Returns the current routes, giving `reason` for nets that have no route for no worse
    /// reason than not having been reached.
    fn current_routes(&self, reason: UnroutedReason) -> RouteSet<S::Node> {
        let mut routes = RouteSet::new();
        for (net, state) in self.nets.iter().zip(&self.states) {
            if !net.needs_routing() {
                routes.set_route(net.id, Route::new());
            } else if let Some(route) = &state.route {
                routes.set_route(net.id, route.clone());
            } else if state.no_path {
                routes.set_unrouted(net.id, UnroutedReason::NoPath);
            } else {
                routes.set_unrouted(net.id, reason);
            }
        }
        routes
    }

    fn collect_overused(&self) -> Vec<OverusedResource<S::Resource>> {
        // A position in `found` for each table entry, plus one, so that nets can be matched
        // to entries in a single pass.
        let mut positions = vec![0u32; self.table.states.len()];
        let mut found = Vec::new();
        for (entry, state) in self.table.states.iter().enumerate() {
            if state.overuse() > 0 {
                found.push(OverusedResource {
                    resource: self.table.resources[entry],
                    occupancy: state.occupancy,
                    capacity: state.capacity,
                    nets: Vec::new(),
                });
                positions[entry] = found.len() as u32;
            }
        }
        if found.is_empty() {
            return found;
        }

        for (net, state) in self.nets.iter().zip(&self.states) {
            for &entry in &state.held {
                let position = positions[entry as usize];
                if position > 0 {
                    found[position as usize - 1].nets.push(net.id);
                }
            }
        }
        for entry in &mut found {
            entry.nets.sort_unstable();
        }
        found.sort_unstable_by_key(|entry| entry.resource);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;
    use crate::net::{AccessPoint, NetOutcome, Terminal};
    use crate::space::graph::{Graph, GraphBuilder};
    use crate::space::grid::Grid;
    use crate::space::{DenseSlots, InternedSlots};
    use core::cell::Cell;
    use core::fmt::Debug;
    use proptest::prelude::*;
    use std::collections::{BTreeMap, BTreeSet};

    // Helpers.

    fn net(id: u32, terminals: &[u32]) -> Net<NodeId> {
        Net {
            id: NetId(id),
            terminals: terminals.iter().map(|&n| Terminal::at(NodeId(n))).collect(),
        }
    }

    fn point(node: u32, cost: u32) -> AccessPoint<NodeId> {
        AccessPoint {
            node: NodeId(node),
            cost: Cost::from(cost),
        }
    }

    fn terminal(points: &[(u32, u32)]) -> Terminal<NodeId> {
        Terminal::new(points.iter().map(|&(n, c)| point(n, c)).collect()).unwrap()
    }

    /// A directed graph with edges of the given costs.
    fn weighted_graph(nodes: u32, edges: &[(u32, u32, u32)]) -> Graph {
        let mut builder = GraphBuilder::new(nodes);
        for &(from, to, cost) in edges {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                .unwrap();
        }
        builder.build()
    }

    /// A directed graph with edges of cost 1.
    fn graph(nodes: u32, edges: &[(u32, u32)]) -> Graph {
        let edges: Vec<_> = edges.iter().map(|&(from, to)| (from, to, 1)).collect();
        weighted_graph(nodes, &edges)
    }

    fn options() -> EngineOptions<'static> {
        EngineOptions::default()
    }

    /// The nodes of a net's route, as plain numbers, in the route's order.
    fn nodes_of(outcome: &Outcome<NodeId, NodeId>, id: u32) -> Vec<u32> {
        let route = outcome.routes.route(NetId(id)).expect("net has a route");
        route.nodes().map(|node| node.0).collect()
    }

    /// Counts what each resource holds in `routes`, without any help from the engine: every net
    /// is a set of resources, gathered from its root and its edges.
    fn occupancy<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        routes: &RouteSet<S::Node>,
    ) -> BTreeMap<S::Resource, BTreeSet<NetId>> {
        let mut holders: BTreeMap<S::Resource, BTreeSet<NetId>> = BTreeMap::new();
        let mut out = Vec::new();
        for net in nets {
            let Some(route) = routes.route(net.id) else {
                continue;
            };
            out.clear();
            if let Some(root) = route.root() {
                space.resources(net.id, root, root, &mut out);
            }
            for edge in route.edges() {
                space.resources(net.id, edge.from, edge.to, &mut out);
            }
            for &resource in &out {
                holders.entry(resource).or_default().insert(net.id);
            }
        }
        holders
    }

    /// Checks each route on its own: it must be a tree grown from the driver over real steps,
    /// reaching every terminal. This walks the edges itself rather than using the checks in
    /// [`crate::net`], and it only looks at the problem and the routes.
    fn check_routes<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        routes: &RouteSet<S::Node>,
    ) -> Result<(), String> {
        for net in nets {
            let Some(outcome) = routes.outcome(net.id) else {
                return Err(format!("{:?} is not in the result", net.id));
            };
            let NetOutcome::Routed(route) = outcome else {
                continue;
            };
            let Some(root) = route.root() else {
                if net.needs_routing() {
                    return Err(format!("{:?} needs a route and has an empty one", net.id));
                }
                continue;
            };
            if !net.needs_routing() {
                return Err(format!("{:?} needs no route", net.id));
            }
            let driver = net.driver().unwrap().access_points();
            if !driver.iter().any(|point| point.node == root) {
                return Err(format!("{:?} is not rooted at its driver", net.id));
            }

            let mut on_tree = HashSet::from([root]);
            let mut steps = Vec::new();
            for edge in route.edges() {
                if !on_tree.contains(&edge.from) {
                    return Err(format!("{:?} has a disconnected {edge:?}", net.id));
                }
                if !on_tree.insert(edge.to) {
                    return Err(format!("{:?} has a loop at {edge:?}", net.id));
                }
                steps.clear();
                space.expand(net.id, edge.from, &mut steps);
                if !steps.iter().any(|step| step.to == edge.to) {
                    return Err(format!(
                        "{:?} takes a step that is not there: {edge:?}",
                        net.id
                    ));
                }
            }
            for (index, terminal) in net.terminals.iter().enumerate() {
                let points = terminal.access_points();
                if !points.iter().any(|point| on_tree.contains(&point.node)) {
                    return Err(format!("{:?} misses terminal {index}", net.id));
                }
            }
        }
        Ok(())
    }

    /// Checks that a result is complete and legal, using nothing from the engine.
    fn check_legal<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        routes: &RouteSet<S::Node>,
    ) -> Result<(), String> {
        check_routes(space, nets, routes)?;
        if let Some((net, reason)) = routes.unrouted().next() {
            return Err(format!("{net:?} is unrouted: {reason}"));
        }
        for (resource, holders) in occupancy(space, nets, routes) {
            if holders.len() as u64 > u64::from(space.capacity(resource)) {
                return Err(format!("{resource:?} is overused by {holders:?}"));
            }
        }
        Ok(())
    }

    /// Checks that an outcome tells the truth about its routes, whatever it says they are.
    fn check_honest<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        outcome: &Outcome<S::Node, S::Resource>,
    ) -> Result<(), String> {
        check_routes(space, nets, &outcome.routes)?;
        let expected: Vec<_> = occupancy(space, nets, &outcome.routes)
            .into_iter()
            .filter(|(resource, holders)| {
                holders.len() as u64 > u64::from(space.capacity(*resource))
            })
            .map(|(resource, holders)| {
                let capacity = space.capacity(resource);
                (resource, holders.len() as u32, capacity, holders)
            })
            .collect();
        let reported: Vec<_> = outcome
            .overused
            .iter()
            .map(|entry| {
                let nets: BTreeSet<NetId> = entry.nets.iter().copied().collect();
                (entry.resource, entry.occupancy, entry.capacity, nets)
            })
            .collect();
        if expected != reported {
            return Err(format!("overuse is {expected:?}, reported {reported:?}"));
        }
        if outcome.is_success() {
            check_legal(space, nets, &outcome.routes)?;
        }
        if outcome.is_success() != outcome.legal_routes().is_some() {
            return Err("is_success and legal_routes disagree".to_string());
        }
        Ok(())
    }

    /// Whether a driver access point exists that reaches every terminal, found by graph
    /// search over the unpriced space. This is what `NoPath` has to agree with.
    fn routable<S: RoutingSpace>(space: &S, net: &Net<S::Node>) -> bool {
        if !net.needs_routing() {
            return true;
        }
        let mut steps = Vec::new();
        net.driver().unwrap().access_points().iter().any(|root| {
            let mut seen = HashSet::from([root.node]);
            let mut stack = vec![root.node];
            while let Some(node) = stack.pop() {
                steps.clear();
                space.expand(net.id, node, &mut steps);
                for step in &steps {
                    if seen.insert(step.to) {
                        stack.push(step.to);
                    }
                }
            }
            net.terminals[1..].iter().all(|terminal| {
                terminal
                    .access_points()
                    .iter()
                    .any(|point| seen.contains(&point.node))
            })
        })
    }

    /// Runs the engine with every check that applies to any result, and returns the outcome.
    fn route_checked<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        options: EngineOptions<'_>,
    ) -> Outcome<S::Node, S::Resource> {
        let outcome = route(space, nets, options).unwrap();
        if let Err(problem) = check_honest(space, nets, &outcome) {
            panic!("{problem}");
        }
        outcome
    }

    fn assert_legal<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        outcome: &Outcome<S::Node, S::Resource>,
    ) where
        S::Node: Debug,
    {
        assert!(outcome.is_success(), "{:?}", outcome.overused);
        assert_eq!(outcome.stop, StopReason::Converged);
        if let Err(problem) = check_legal(space, nets, &outcome.routes) {
            panic!("{problem}");
        }
    }

    /// A random-order permutation of `0..n`, from a seed.
    fn permutation(n: u32, seed: u64) -> Vec<u32> {
        let mut items: Vec<u32> = (0..n).collect();
        crate::rng::Rng::new(seed).shuffle(&mut items);
        items
    }

    /// FNV-1a over 32-bit words, to pin routes to a value that any platform can compute. Each word
    /// is fed in as four little-endian bytes.
    fn hash_words(hash: &mut u64, word: u32) {
        for byte in word.to_le_bytes() {
            *hash ^= u64::from(byte);
            *hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    fn hash_outcome(outcome: &Outcome<NodeId, NodeId>) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325;
        for (id, result) in outcome.routes.outcomes() {
            hash_words(&mut hash, id.0);
            match result {
                NetOutcome::Routed(route) => {
                    hash_words(&mut hash, route.edges().len() as u32);
                    hash_words(&mut hash, route.root().map_or(u32::MAX, |node| node.0));
                    for edge in route.edges() {
                        hash_words(&mut hash, edge.from.0);
                        hash_words(&mut hash, edge.to.0);
                    }
                }
                NetOutcome::Unrouted(_) => hash_words(&mut hash, u32::MAX),
            }
        }
        for entry in &outcome.overused {
            hash_words(&mut hash, entry.resource.0);
            hash_words(&mut hash, entry.occupancy);
        }
        hash
    }

    // Crossings and congestion.

    #[test]
    fn nets_that_share_nothing_finish_in_one_iteration() {
        let grid = Grid::new(4, 4, 1).unwrap();
        let nets = [net(0, &[0, 3]), net(1, &[12, 15])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(outcome.routes.stats.iterations, 1);
        assert_eq!(outcome.best_iteration, 1);
        assert_eq!(outcome.total_cost, Cost::from(6));
        assert_eq!(nodes_of(&outcome, 0), [0, 1, 2, 3]);
    }

    #[test]
    fn crossing_nets_resolve_with_a_layer_change() {
        // Both straight lines cross at the centre, so the first pass must conflict.
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];

        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        let first = engine.step().unwrap();
        assert_eq!(first.total_overuse, 1);
        assert_eq!(first.overused_resources, 1);
        assert_eq!(first.nets_rerouted, 2);
        let overused = engine.overused();
        assert_eq!(overused[0].resource, grid.node_at(2, 2, 0).unwrap());
        assert_eq!(overused[0].nets, [NetId(0), NetId(1)]);

        let outcome = engine.run();
        assert_legal(&grid, &nets, &outcome);
        assert!(outcome.routes.stats.iterations >= 2);
    }

    #[test]
    fn a_terminal_on_another_nets_shortest_path_pushes_that_net_away() {
        // One layer, so net 0 must go all the way around net 1, which cannot move off its
        // terminals. The obvious first pass runs net 0 straight through net 1's driver.
        let grid = Grid::new(4, 4, 1).unwrap();
        let nets = [net(0, &[0, 3]), net(1, &[1, 9])];
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        assert!(engine.step().unwrap().total_overuse > 0);
        let outcome = engine.run();
        assert_legal(&grid, &nets, &outcome);
        assert!(!nodes_of(&outcome, 0).contains(&1));
    }

    #[test]
    fn the_earlier_net_gives_way_when_only_the_later_net_has_no_choice() {
        // Net 0 prefers the bottleneck 2 but can use 3. Net 1 has only the bottleneck.
        let space = weighted_graph(
            7,
            &[
                (0, 2, 1),
                (2, 4, 1),
                (0, 3, 2),
                (3, 4, 2),
                (1, 2, 1),
                (2, 5, 1),
            ],
        );
        let nets = [net(0, &[0, 4]), net(1, &[1, 5])];
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [0, 3, 4]);
        assert_eq!(nodes_of(&outcome, 1), [1, 2, 5]);
        assert_eq!(outcome.total_cost, Cost::from(6));
    }

    #[test]
    fn zero_cost_steps_still_get_negotiated() {
        // A product with the base cost could never make these free steps dearer.
        let space = weighted_graph(
            6,
            &[
                (0, 2, 0),
                (2, 4, 0),
                (0, 3, 1),
                (3, 4, 0),
                (1, 2, 0),
                (2, 5, 0),
            ],
        );
        let nets = [net(0, &[0, 4]), net(1, &[1, 5])];
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
    }

    #[test]
    fn only_nets_on_overused_resources_are_routed_again() {
        let grid = Grid::new(5, 6, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            net(0, &[at(0, 2), at(4, 2)]),
            net(1, &[at(2, 0), at(2, 4)]),
            net(2, &[at(0, 5), at(4, 5)]), // Well away from the other two.
        ];
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().nets_rerouted, 3);
        let second = engine.step().unwrap();
        assert!(second.nets_rerouted <= 2);
        assert!(engine.states[2].cost == Cost::from(4));
    }

    #[test]
    fn a_conflict_settled_earlier_in_the_iteration_does_not_reroute_the_other_net() {
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        // A price high enough to make the first net to go move at once.
        let dear = EngineOptions {
            initial_present: Cost::from(10),
            ..options()
        };
        let mut engine = Engine::new(&grid, &nets, dear).unwrap();
        engine.step();
        // Net 0 moves off the shared cell, and that leaves net 1 with nothing to resolve.
        let second = engine.step().unwrap();
        assert_eq!(second.total_overuse, 0);
        assert_eq!(second.nets_rerouted, 1);
    }

    #[test]
    fn full_reroute_visits_every_net_every_time() {
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        let reroute_all = EngineOptions {
            reroute_all: true,
            ..options()
        };
        let mut engine = Engine::new(&grid, &nets, reroute_all).unwrap();
        while let Some(report) = engine.step() {
            assert_eq!(report.nets_rerouted, 2);
        }
        assert_legal(&grid, &nets, &engine.outcome());
    }

    // Capacity.

    #[test]
    fn capacity_above_one_is_shared_up_to_the_limit() {
        // Three nets through a middle node 3, which has room for all of them or not.
        let edges = [(0, 3), (1, 3), (2, 3), (3, 4), (3, 5), (3, 6)];
        let nets = [net(0, &[0, 4]), net(1, &[1, 5]), net(2, &[2, 6])];
        let with_capacity = |capacity| {
            let mut builder = GraphBuilder::new(7);
            for &(from, to) in &edges {
                builder
                    .add_edge(NodeId(from), NodeId(to), Cost::ONE)
                    .unwrap();
            }
            builder.set_capacity(NodeId(3), capacity).unwrap();
            builder.build()
        };

        let roomy = with_capacity(3);
        let outcome = route_checked(&roomy, &nets, options());
        assert_legal(&roomy, &nets, &outcome);
        assert_eq!(outcome.routes.stats.iterations, 1);

        let tight = with_capacity(2);
        let outcome = route_checked(&tight, &nets, options());
        assert!(!outcome.is_success());
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].occupancy, 3);
        assert_eq!(outcome.overused[0].capacity, 2);
    }

    #[test]
    fn capacity_two_channels_carry_two_nets_each() {
        // A corridor one cell wide with capacity 2, and four nets to send through two corridors.
        let mut grid = Grid::new(5, 3, 1).unwrap();
        for x in 1..4 {
            grid.set_capacity(x, 0, 0, 2).unwrap();
            grid.set_capacity(x, 2, 0, 2).unwrap();
            grid.set_blocked(x, 1, 0, true).unwrap();
        }
        for y in [0, 2] {
            grid.set_capacity(0, y, 0, 2).unwrap();
            grid.set_capacity(4, y, 0, 2).unwrap();
        }
        grid.set_capacity(0, 1, 0, 4).unwrap();
        grid.set_capacity(4, 1, 0, 4).unwrap();
        let from = grid.node_at(0, 1, 0).unwrap().0;
        let to = grid.node_at(4, 1, 0).unwrap().0;
        let nets: Vec<_> = (0..4).map(|id| net(id, &[from, to])).collect();
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
    }

    #[test]
    fn a_zero_capacity_resource_is_avoided_from_the_very_first_pass() {
        // The short way is through node 1, where any use is overuse. A search that assumed that
        // an untouched resource is free would need a second iteration to find that out.
        let mut builder = GraphBuilder::new(4);
        for (from, to, cost) in [(0, 1, 1), (1, 3, 1), (0, 2, 2), (2, 3, 2)] {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                .unwrap();
        }
        builder.set_capacity(NodeId(1), 0).unwrap();
        let space = builder.build();
        let nets = [net(0, &[0, 3])];

        // The first pass charges nothing for congestion, so it takes a price to see this.
        let priced_first = EngineOptions {
            first_present: Cost::from(10),
            ..options()
        };
        let outcome = route_checked(&space, &nets, priced_first);
        assert_legal(&space, &nets, &outcome);
        assert_eq!(outcome.routes.stats.iterations, 1);
        assert_eq!(nodes_of(&outcome, 0), [0, 2, 3]);

        // And by default it takes one more iteration, but gets there.
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [0, 2, 3]);
    }

    // Occupancy.

    /// A space whose steps consume a shared bus as well as the node they arrive at, which is how
    /// one net can consume a resource over and over.
    struct BusSpace {
        graph: Graph,
        bus_capacity: u32,
        reports_bus_twice: bool,
        root_resource: bool,
    }

    const BUS: NodeId = NodeId(1000);
    const ROOT_PREFIX: u32 = 2000;

    impl RoutingSpace for BusSpace {
        type Node = NodeId;
        type Resource = NodeId;
        type Slots = DenseSlots<NodeId>;

        fn new_slots(&self) -> Self::Slots {
            DenseSlots::new()
        }

        fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
            self.graph.expand(net, from, out);
        }

        fn lower_bound(&self, _net: NetId, _from: NodeId, _to: NodeId) -> Cost {
            Cost::ZERO
        }

        fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
            if from == to {
                if self.root_resource {
                    out.push(NodeId(ROOT_PREFIX + to.0));
                }
                return;
            }
            out.push(BUS);
            out.push(to);
            if self.reports_bus_twice {
                out.push(BUS);
            }
            let _ = net;
        }

        fn capacity(&self, resource: NodeId) -> u32 {
            if resource == BUS {
                self.bus_capacity
            } else {
                1
            }
        }
    }

    #[test]
    fn a_net_counts_once_on_a_resource_used_by_many_edges() {
        for reports_bus_twice in [false, true] {
            let space = BusSpace {
                graph: graph(4, &[(0, 1), (1, 2), (2, 3)]),
                bus_capacity: 1,
                reports_bus_twice,
                root_resource: false,
            };
            let nets = [net(0, &[0, 3])];
            let outcome = route_checked(&space, &nets, options());
            assert_legal(&space, &nets, &outcome);
            assert_eq!(outcome.routes.stats.iterations, 1);
        }
    }

    #[test]
    fn two_nets_on_a_shared_bus_are_each_counted_once() {
        let space = BusSpace {
            graph: graph(6, &[(0, 1), (1, 2), (3, 4), (4, 5)]),
            bus_capacity: 1,
            reports_bus_twice: false,
            root_resource: false,
        };
        let nets = [net(0, &[0, 2]), net(1, &[3, 5])];
        let outcome = route_checked(&space, &nets, options());
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].resource, BUS);
        assert_eq!(outcome.overused[0].occupancy, 2);
        assert_eq!(outcome.overused[0].nets, [NetId(0), NetId(1)]);
    }

    #[test]
    fn a_growing_tree_is_not_charged_for_what_it_already_holds() {
        /// Steps consume a bus whose capacity is zero, except those on the long way through
        /// node 5. A net that holds the bus already should not pay to go on using it.
        struct OccupiedBus(Graph);
        impl RoutingSpace for OccupiedBus {
            type Node = NodeId;
            type Resource = NodeId;
            type Slots = DenseSlots<NodeId>;
            fn new_slots(&self) -> Self::Slots {
                DenseSlots::new()
            }
            fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
                self.0.expand(net, from, out);
            }
            fn lower_bound(&self, _: NetId, _: NodeId, _: NodeId) -> Cost {
                Cost::ZERO
            }
            fn resources(&self, _: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
                if from != to && from != NodeId(5) && to != NodeId(5) {
                    out.push(BUS);
                }
            }
            fn capacity(&self, _: NodeId) -> u32 {
                0
            }
        }

        // The first path has to take 0 -> 1, and so the bus. After that, 1 -> 2 costs 1 and
        // 1 -> 5 -> 2 costs 4, so a charge for the bus would send the net the long way.
        let inner = weighted_graph(6, &[(0, 1, 1), (1, 2, 1), (1, 5, 2), (5, 2, 2)]);
        let space = OccupiedBus(inner);
        let nets = [net(0, &[0, 1, 2])];
        let priced = EngineOptions {
            first_present: Cost::from(100),
            max_iterations: 1,
            ..options()
        };
        let outcome = route(&space, &nets, priced).unwrap();
        assert_eq!(nodes_of(&outcome, 0), [0, 1, 2]);
        assert_eq!(outcome.total_cost, Cost::from(2));
    }

    #[test]
    fn the_root_counts_as_occupied() {
        // Both nets start at node 0, which only one of them can hold. No edge consumes it, so
        // only the call with the root at both ends tells the engine.
        let space = graph(3, &[(0, 1), (0, 2)]);
        let nets = [net(0, &[0, 1]), net(1, &[0, 2])];
        let outcome = route_checked(&space, &nets, options());
        assert!(!outcome.is_success());
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].resource, NodeId(0));
        assert_eq!(outcome.overused[0].nets, [NetId(0), NetId(1)]);
    }

    #[test]
    fn a_congested_root_is_negotiated_away_when_the_driver_has_another() {
        // Net 1 must start at node 0, and net 0 can use either node 0 or the dearer node 1.
        let space = graph(4, &[(0, 2), (1, 2), (0, 3)]);
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![terminal(&[(0, 0), (1, 1)]), Terminal::at(NodeId(2))],
            },
            net(1, &[0, 3]),
        ];
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().total_overuse, 1);
        let outcome = engine.run();
        assert_legal(&space, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [1, 2]);
        assert_eq!(outcome.total_cost, Cost::from(3));
    }

    // Unroutable nets and impossible problems.

    #[test]
    fn a_net_with_no_path_is_reported_and_others_still_route() {
        let grid = {
            let mut grid = Grid::new(5, 5, 1).unwrap();
            for y in 0..5 {
                grid.set_blocked(2, y, 0, true).unwrap();
            }
            grid
        };
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            net(0, &[at(0, 0), at(1, 4)]),
            net(1, &[at(0, 2), at(4, 2)]), // Across the wall.
            net(2, &[at(3, 0), at(4, 4)]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert!(!outcome.is_success());
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(1), UnroutedReason::NoPath)]
        );
        assert_eq!(outcome.routes.routes().count(), 2);
        assert!(outcome.overused.is_empty());
        assert_eq!(outcome.stop, StopReason::Converged);
        assert!(outcome.legal_routes().is_none());
    }

    #[test]
    fn a_net_with_no_path_is_searched_for_once() {
        // Nets 1 and 2 conflict for a while, and that must not repeat net 0's failed search,
        // which has to flood everything it can reach to prove anything.
        let space = graph(8, &[(0, 1), (2, 4), (3, 4), (4, 5), (4, 6), (2, 7), (7, 5)]);
        let nets = [net(0, &[0, 6]), net(1, &[2, 5]), net(2, &[3, 6])];
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().searches, 3);
        while let Some(report) = engine.step() {
            assert!(report.searches <= 2);
        }
    }

    #[test]
    fn a_directed_dead_end_tells_a_blocked_way_from_a_bad_root() {
        // The driver has two access points. From node 0, the near terminal is 2 but the far one
        // cannot be reached, whereas node 1 reaches both, at a higher cost. A router that keeps
        // to its first choice of root would wrongly report no path.
        let space = weighted_graph(5, &[(0, 2, 1), (1, 2, 5), (1, 3, 5)]);
        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                terminal(&[(0, 0), (1, 0)]),
                Terminal::at(NodeId(2)),
                Terminal::at(NodeId(3)),
            ],
        }];
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().searches, 4);
        assert_eq!(engine.states[0].dead_roots, [NodeId(0)]);
        let outcome = engine.run();
        assert_legal(&space, &nets, &outcome);
        assert_eq!(
            outcome.routes.route(NetId(0)).unwrap().root(),
            Some(NodeId(1))
        );
    }

    #[test]
    fn no_root_reaching_everything_is_no_path_even_if_each_terminal_is_reachable() {
        // Each root reaches one terminal, and neither reaches both.
        let space = graph(4, &[(0, 2), (1, 3)]);
        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                terminal(&[(0, 0), (1, 0)]),
                Terminal::at(NodeId(2)),
                Terminal::at(NodeId(3)),
            ],
        }];
        let outcome = route_checked(&space, &nets, options());
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(0), UnroutedReason::NoPath)]
        );
        assert_eq!(outcome.total_cost, Cost::ZERO);
    }

    #[test]
    fn too_little_capacity_terminates_and_says_so() {
        // Three nets through one node, on a problem with no other way across.
        let space = graph(7, &[(0, 3), (1, 3), (2, 3), (3, 4), (3, 5), (3, 6)]);
        let nets = [net(0, &[0, 4]), net(1, &[1, 5]), net(2, &[2, 6])];
        let limited = EngineOptions {
            max_iterations: 20,
            ..options()
        };
        let outcome = route_checked(&space, &nets, limited);

        assert_eq!(outcome.stop, StopReason::IterationLimit);
        assert_eq!(outcome.routes.stats.iterations, 20);
        assert!(!outcome.is_success());
        assert!(outcome.legal_routes().is_none());
        assert_eq!(outcome.routes.routes().count(), 3);
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].resource, NodeId(3));
        assert_eq!(outcome.overused[0].occupancy, 3);
        assert_eq!(outcome.overused[0].capacity, 1);
        assert_eq!(outcome.conflicted_nets(), [NetId(0), NetId(1), NetId(2)]);
    }

    #[test]
    fn too_little_capacity_on_a_grid_keeps_the_best_iteration() {
        // A wall with one gap, and more nets than the gap can take. They stay in conflict
        // whatever happens, with the best iteration being the first of the least overuse.
        let mut grid = Grid::new(5, 6, 1).unwrap();
        for y in 0..6 {
            if y != 3 {
                grid.set_blocked(2, y, 0, true).unwrap();
            }
        }
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets: Vec<_> = (0..4).map(|y| net(y, &[at(0, y), at(4, y)])).collect();
        let limited = EngineOptions {
            max_iterations: 30,
            ..options()
        };
        let mut engine = Engine::new(&grid, &nets, limited).unwrap();
        while engine.step().is_some() {}
        let outcome = engine.outcome();
        assert!(check_honest(&grid, &nets, &outcome).is_ok());
        assert!(!outcome.is_success());
        assert_eq!(outcome.stop, StopReason::IterationLimit);

        // The ordering is exactly as documented.
        let reports = engine.reports();
        let rank = |r: &IterationReport| (r.unrouted, r.total_overuse, r.total_cost);
        let best = reports.iter().map(rank).min().unwrap();
        let first_best = reports.iter().position(|r| rank(r) == best).unwrap();
        assert_eq!(outcome.best_iteration, first_best as u32 + 1);
        assert_eq!(outcome.total_cost, best.2);
    }

    #[test]
    fn best_prefers_fewer_unrouted_over_less_overuse() {
        let (best, candidate) = ((1u32, 0u64, Cost::ONE), (0u32, 9u64, Cost::MAX));
        assert!(candidate < best);
    }

    // Multi-terminal nets.

    #[test]
    fn a_tree_grows_to_each_terminal_with_shared_trunk() {
        // A corridor along the middle row, with one stub up and one stub down.
        let mut grid = Grid::new(7, 3, 1).unwrap();
        for x in 0..7 {
            if x != 3 {
                grid.set_blocked(x, 0, 0, true).unwrap();
            }
            if x != 5 {
                grid.set_blocked(x, 2, 0, true).unwrap();
            }
        }
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 1), at(6, 1), at(3, 0), at(5, 2)])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        // Six along the corridor, shared by all three paths, and the two stubs.
        assert_eq!(outcome.total_cost, Cost::from(8));
        assert_eq!(outcome.routes.route(NetId(0)).unwrap().edges().len(), 8);
    }

    #[test]
    fn a_path_passing_over_a_terminal_connects_it_without_a_search() {
        let grid = Grid::new(5, 1, 1).unwrap();
        let nets = [net(0, &[0, 4, 2])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        // The nearest terminal is 2, then a search from the tree finishes at 4.
        assert_eq!(outcome.routes.stats.searches, 2);

        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                Terminal::at(NodeId(0)),
                terminal(&[(4, 0)]),
                terminal(&[(2, 100)]),
            ],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        // A long way to 4 passes over 2 for free, leaving nothing else to do.
        assert_eq!(outcome.routes.stats.searches, 1);
        assert_eq!(outcome.total_cost, Cost::from(104));
    }

    #[test]
    fn terminals_on_one_node_need_nothing_but_a_root() {
        let grid = Grid::new(3, 1, 1).unwrap();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                terminal(&[(0, 1), (1, 0)]),
                terminal(&[(1, 0), (2, 0)]),
                terminal(&[(1, 2)]),
            ],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(
            outcome.routes.route(NetId(0)),
            Some(&Route::from_root(NodeId(1)))
        );
        assert_eq!(outcome.total_cost, Cost::from(2));
    }

    #[test]
    fn single_and_empty_nets_get_empty_routes_and_occupy_nothing() {
        let grid = Grid::new(3, 1, 1).unwrap();
        let nets = [
            net(0, &[1]),
            Net {
                id: NetId(1),
                terminals: vec![],
            },
            net(2, &[0, 2]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(outcome.routes.route(NetId(0)), Some(&Route::new()));
        assert_eq!(outcome.routes.route(NetId(1)), Some(&Route::new()));
        assert_eq!(outcome.routes.stats.searches, 1);
    }

    #[test]
    fn multi_terminal_nets_negotiate_like_any_other() {
        let grid = Grid::new(6, 6, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            net(0, &[at(0, 0), at(5, 5), at(0, 5)]),
            net(1, &[at(5, 0), at(0, 2), at(3, 5)]),
            net(2, &[at(2, 0), at(2, 5), at(5, 3)]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
    }

    // Equivalent access points.

    #[test]
    fn any_access_point_of_a_terminal_satisfies_it() {
        let grid = Grid::new(6, 1, 1).unwrap();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![Terminal::at(NodeId(2)), terminal(&[(5, 0), (3, 0)])],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [2, 3]);
    }

    #[test]
    fn access_costs_steer_the_choice_of_access_point() {
        let grid = Grid::new(10, 1, 1).unwrap();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![Terminal::at(NodeId(2)), terminal(&[(3, 9), (7, 0)])],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_eq!(nodes_of(&outcome, 0), [2, 3, 4, 5, 6, 7]);
        assert_eq!(outcome.total_cost, Cost::from(5));

        // At the driver too: starting further away costs less.
        let nets = [Net {
            id: NetId(0),
            terminals: vec![terminal(&[(2, 9), (8, 1)]), Terminal::at(NodeId(5))],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_eq!(nodes_of(&outcome, 0), [8, 7, 6, 5]);
        assert_eq!(outcome.total_cost, Cost::from(4));
    }

    #[test]
    fn congestion_moves_a_terminal_to_another_access_point() {
        // Net 0's nearer access point at 2 lies on net 1's only way.
        let grid = Grid::new(5, 3, 1).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![
                    Terminal::at(NodeId(at(0, 1))),
                    terminal(&[(at(1, 1), 0), (at(0, 0), 3)]),
                ],
            },
            net(1, &[at(1, 0), at(1, 2)]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [at(0, 1), at(0, 0)]);
    }

    // Budgets and cancellation.

    #[test]
    fn a_cancel_flag_already_set_stops_before_any_work() {
        let grid = Grid::new(4, 4, 1).unwrap();
        let flag = AtomicBool::new(true);
        let nets = [net(0, &[0, 3]), net(1, &[5]), net(2, &[0, 15])];
        let cancelled = EngineOptions {
            cancel: Some(&flag),
            ..options()
        };

        let mut engine = Engine::new(&grid, &nets, cancelled).unwrap();
        assert_eq!(engine.step(), None);
        assert_eq!(engine.stop_reason(), Some(StopReason::Cancelled));
        let outcome = engine.outcome();

        assert_eq!(outcome.stop, StopReason::Cancelled);
        assert_eq!(outcome.routes.stats, RouteStats::default());
        assert_eq!(outcome.best_iteration, 0);
        assert!(!outcome.is_success());
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [
                (NetId(0), UnroutedReason::Cancelled),
                (NetId(2), UnroutedReason::Cancelled)
            ]
        );
        // Net 1 needs no routing, so that is no failure.
        assert_eq!(outcome.routes.route(NetId(1)), Some(&Route::new()));
    }

    /// A grid that raises a flag once a given number of expansions have gone by.
    struct Tripwire<'a> {
        grid: Grid,
        flag: &'a AtomicBool,
        left: Cell<u32>,
    }

    impl RoutingSpace for Tripwire<'_> {
        type Node = NodeId;
        type Resource = NodeId;
        type Slots = DenseSlots<NodeId>;

        fn new_slots(&self) -> Self::Slots {
            DenseSlots::new()
        }

        fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
            if self.left.get() == 0 {
                self.flag.store(true, Ordering::Relaxed);
            } else {
                self.left.set(self.left.get() - 1);
            }
            self.grid.expand(net, from, out);
        }

        fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
            self.grid.lower_bound(net, from, to)
        }

        fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
            self.grid.resources(net, from, to, out);
        }

        fn capacity(&self, resource: NodeId) -> u32 {
            self.grid.capacity(resource)
        }
    }

    #[test]
    fn cancelling_part_way_keeps_routes_and_the_best_so_far() {
        let flag = AtomicBool::new(false);
        let space = Tripwire {
            grid: Grid::new(5, 5, 2).unwrap(),
            flag: &flag,
            left: Cell::new(u32::MAX),
        };
        let at = |x, y| space.grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        let cancelled = EngineOptions {
            cancel: Some(&flag),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, cancelled).unwrap();
        let first = engine.step().unwrap();
        assert_eq!(first.total_overuse, 1);

        // The flag goes up during the next search, which is too short to notice. So the
        // search finishes and the iteration stops before the following net.
        space.left.set(0);
        let second = engine.step().unwrap();
        assert!(!second.completed);
        assert_eq!(engine.stop_reason(), Some(StopReason::Cancelled));
        assert_eq!(engine.step(), None);

        // Every net still has its route from before, including the one interrupted.
        let outcome = engine.outcome();
        assert_eq!(outcome.stop, StopReason::Cancelled);
        assert_eq!(outcome.routes.routes().count(), 2);
        assert_eq!(outcome.overused.len(), 1);
        assert!(check_honest(&space, &nets, &outcome).is_ok());
        assert!(engine.table.states.iter().map(|s| s.occupancy).sum::<u32>() > 0);
        let held: u32 = engine.states.iter().map(|s| s.held.len() as u32).sum();
        assert_eq!(
            engine.table.states.iter().map(|s| s.occupancy).sum::<u32>(),
            held
        );
    }

    #[test]
    fn cancelling_during_the_first_iteration_says_which_nets_it_cut_off() {
        let flag = AtomicBool::new(false);
        let space = Tripwire {
            grid: Grid::new(6, 2, 1).unwrap(),
            flag: &flag,
            left: Cell::new(6),
        };
        let nets = [net(0, &[0, 5]), net(1, &[6, 11])];
        let cancelled = EngineOptions {
            cancel: Some(&flag),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, cancelled).unwrap();
        let report = engine.step().unwrap();
        assert!(!report.completed);
        let outcome = engine.outcome();
        assert_eq!(outcome.stop, StopReason::Cancelled);
        assert_eq!(outcome.routes.route(NetId(0)).unwrap().edges().len(), 5);
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(1), UnroutedReason::Cancelled)]
        );
        assert!(!outcome.is_success());
    }

    #[test]
    fn a_budget_of_nothing_is_not_mistaken_for_no_path() {
        let grid = Grid::new(4, 4, 1).unwrap();
        let nets = [net(0, &[0, 3]), net(1, &[5]), net(2, &[6, 6])];
        let budget = EngineOptions {
            max_expansions: 0,
            ..options()
        };
        let outcome = route_checked(&grid, &nets, budget);
        assert_eq!(outcome.stop, StopReason::BudgetExhausted);
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [
                (NetId(0), UnroutedReason::BudgetExhausted),
                (NetId(2), UnroutedReason::BudgetExhausted)
            ]
        );
    }

    #[test]
    fn a_budget_stops_at_the_limit_and_reports_truthfully() {
        let grid = Grid::new(10, 10, 3).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets: Vec<_> = (0..10).map(|y| net(y, &[at(0, y), at(9, 9 - y)])).collect();
        let full = route_checked(&grid, &nets, options());
        let needed = full.routes.stats.nodes_expanded;
        assert!(full.is_success());

        // Exactly what was needed is enough, and gives the same result.
        let exact = EngineOptions {
            max_expansions: needed,
            ..options()
        };
        assert_eq!(route_checked(&grid, &nets, exact), full);

        // With less, it ends with an honest result that is never over budget.
        for divisor in [2, 3, 7, 50, 1000] {
            let budget = needed / divisor;
            let limited = EngineOptions {
                max_expansions: budget,
                ..options()
            };
            let outcome = route_checked(&grid, &nets, limited);
            assert_eq!(outcome.stop, StopReason::BudgetExhausted);
            assert!(outcome.routes.stats.nodes_expanded <= budget);
            assert!(
                outcome
                    .routes
                    .unrouted()
                    .all(|(_, reason)| reason == UnroutedReason::BudgetExhausted)
            );
        }
    }

    #[test]
    fn an_iteration_limit_of_zero_routes_nothing_and_says_why() {
        let grid = Grid::new(4, 1, 1).unwrap();
        let nets = [net(0, &[0, 3])];
        let none = EngineOptions {
            max_iterations: 0,
            ..options()
        };
        let outcome = route_checked(&grid, &nets, none);
        assert_eq!(outcome.stop, StopReason::IterationLimit);
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(0), UnroutedReason::BudgetExhausted)]
        );
    }

    // Stepping.

    #[test]
    fn stepping_matches_running_and_reports_each_iteration() {
        let grid = Grid::new(8, 8, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets: Vec<_> = (0..8).map(|y| net(y, &[at(0, y), at(7, 7 - y)])).collect();

        let run = route(&grid, &nets, options()).unwrap();

        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        assert_eq!(engine.stop_reason(), None);
        assert_eq!(engine.outcome().stop, StopReason::NotFinished);
        let mut count = 0;
        let (mut searches, mut expanded) = (0, 0);
        while let Some(report) = engine.step() {
            count += 1;
            assert_eq!(report.iteration, count);
            assert!(report.completed);
            searches += report.searches;
            expanded += report.nodes_expanded;
            // Observable, and consistent with what the report says.
            let overused = engine.overused();
            assert_eq!(overused.len() as u32, report.overused_resources);
            let total: u64 = overused
                .iter()
                .map(|entry| u64::from(entry.occupancy - entry.capacity))
                .sum();
            assert_eq!(total, report.total_overuse);
            assert_eq!(report.unrouted, 0);
        }
        assert_eq!(engine.step(), None);
        assert_eq!(engine.reports().len() as u32, count);

        let stepped = engine.outcome();
        assert_eq!(stepped, run);
        assert_eq!(stepped.routes.stats.iterations, u64::from(count));
        assert_eq!(stepped.routes.stats.searches, searches);
        assert_eq!(stepped.routes.stats.nodes_expanded, expanded);
    }

    #[test]
    fn stopping_between_iterations_keeps_the_best_so_far() {
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        engine.step();
        let outcome = engine.outcome();
        assert_eq!(outcome.stop, StopReason::NotFinished);
        assert_eq!(outcome.best_iteration, 1);
        assert_eq!(outcome.routes.routes().count(), 2);
        assert_eq!(outcome.overused.len(), 1);
        assert!(outcome.legal_routes().is_none());
        assert!(check_honest(&grid, &nets, &outcome).is_ok());
    }

    #[test]
    fn the_present_factor_follows_its_schedule_up_to_the_cap() {
        let space = graph(4, &[(0, 2), (1, 2), (2, 3)]);
        let nets = [net(0, &[0, 3]), net(1, &[1, 3])];
        let schedule = EngineOptions {
            max_iterations: 8,
            first_present: Cost::ZERO,
            initial_present: Cost::from(2),
            present_growth: Cost::from(3),
            max_present: Cost::from(100),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, schedule).unwrap();
        let mut seen = Vec::new();
        while let Some(report) = engine.step() {
            seen.push(report.present.get());
        }
        assert_eq!(seen, [0.0, 2.0, 6.0, 18.0, 54.0, 100.0, 100.0, 100.0]);
    }

    #[test]
    fn history_accumulates_only_on_overused_resources() {
        // Node 2 is overused by one, with capacity two and three nets on it, until the end.
        let mut builder = GraphBuilder::new(7);
        for (from, to) in [(0, 3), (1, 3), (2, 3), (3, 4), (3, 5), (3, 6)] {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::ONE)
                .unwrap();
        }
        builder.set_capacity(NodeId(3), 2).unwrap();
        let space = builder.build();
        let nets = [net(0, &[0, 4]), net(1, &[1, 5]), net(2, &[2, 6])];
        let schedule = EngineOptions {
            max_iterations: 3,
            history_factor: Cost::from(5),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, schedule).unwrap();
        engine.step();
        engine.step();
        let history = |engine: &Engine<'_, Graph>, node| {
            engine.table.states[engine.table.index[&NodeId(node)] as usize].history
        };
        assert_eq!(history(&engine, 3), Cost::from(10));
        assert_eq!(history(&engine, 4), Cost::ZERO);
        // None is added for the iteration that ends the run, as nothing would ever read it.
        engine.step();
        assert_eq!(engine.stop_reason(), Some(StopReason::IterationLimit));
        assert_eq!(history(&engine, 3), Cost::from(10));
    }

    // The pricing wrapper.

    #[test]
    fn prices_follow_the_formula() {
        let space = BusSpace {
            graph: graph(4, &[(0, 1), (1, 2), (0, 2)]),
            bus_capacity: 2,
            reports_bus_twice: false,
            root_resource: false,
        };
        let mut table = ResourceTable::new();
        let bus = table.intern(&space, BUS);
        let two = table.intern(&space, NodeId(2));
        table.states[bus as usize].occupancy = 3;
        table.states[bus as usize].history = Cost::from(2);
        table.states[two as usize].occupancy = 1;
        table.states[two as usize].history = Cost::from(5);

        let held = FastSet::default();
        let buffer = RefCell::new(Vec::new());
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
        };

        let mut out = vec![Step {
            to: NodeId(77),
            cost: Cost::ONE,
        }];
        priced.expand(NetId(0), NodeId(0), &mut out);
        // Entries already there are left alone.
        assert_eq!(out[0].cost, Cost::ONE);
        // To node 1: the bus alone is in the table, at occupancy 3 of 2 and with history 2, so
        // (1 + 2) * (1 + 10 * 2). A resource missing from the table adds nothing.
        assert_eq!(out[1].to, NodeId(1));
        assert_eq!(out[1].cost, Cost::from(63));
        // To node 2: the bus and node 2 add up. Overuse 2 + 1, history 2 + 5.
        assert_eq!(out[2].to, NodeId(2));
        assert_eq!(out[2].cost, Cost::from(8 * 31));

        // What the net already holds is priced as if it were not there.
        let mut held = FastSet::default();
        held.insert(BUS);
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
        };
        let mut out = Vec::new();
        priced.expand(NetId(0), NodeId(0), &mut out);
        assert_eq!(out[0].cost, Cost::ONE);
        assert_eq!(out[1].cost, Cost::from(6 * 11));
    }

    #[test]
    fn a_resource_listed_twice_for_one_step_is_priced_once() {
        let space = BusSpace {
            graph: graph(2, &[(0, 1)]),
            bus_capacity: 1,
            reports_bus_twice: true,
            root_resource: false,
        };
        let mut table = ResourceTable::new();
        let bus = table.intern(&space, BUS);
        table.states[bus as usize].occupancy = 1;
        table.states[bus as usize].history = Cost::from(2);
        let held = FastSet::default();
        let buffer = RefCell::new(Vec::new());
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
        };
        let mut out = Vec::new();
        priced.expand(NetId(0), NodeId(0), &mut out);
        // Overuse 1 and history 2, and not twice either of them.
        assert_eq!(out[0].cost, Cost::from(3 * 11));
    }

    #[test]
    fn prices_never_fall_below_the_base_cost() {
        let values = [0, 1, 2, 7, 1000];
        let space = graph(1, &[]);
        for base in values {
            for history in values {
                for occupancy in values {
                    for present in values {
                        let mut table = ResourceTable::new();
                        let entry = table.intern(&space, NodeId(0));
                        table.states[entry as usize].history = Cost::from(history);
                        table.states[entry as usize].occupancy = occupancy;
                        let held = FastSet::default();
                        let buffer = RefCell::new(Vec::new());
                        let priced = Priced {
                            inner: &space,
                            table: &table,
                            present: Cost::from(present),
                            held: &held,
                            buffer: &buffer,
                        };
                        let price = priced.price(Cost::from(base), &mut vec![NodeId(0)]);
                        assert!(price >= Cost::from(base));
                    }
                }
            }
        }
    }

    #[test]
    fn hasher_treats_every_key_shape_consistently() {
        let hash = |value: &dyn Fn(&mut FastHasher)| {
            let mut hasher = FastHasher::default();
            value(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash(&|h| h.write_u32(7)), hash(&|h| h.write_u64(7)));
        assert_eq!(hash(&|h| h.write_u8(7)), hash(&|h| h.write_u16(7)));
        assert_eq!(hash(&|h| h.write_usize(7)), hash(&|h| h.write_u64(7)));
        assert_ne!(hash(&|h| h.write_u32(7)), hash(&|h| h.write_u32(8)));
        assert_eq!(
            hash(&|h| h.write(&[7, 0, 0, 0, 0, 0, 0, 0])),
            hash(&|h| h.write_u64(7))
        );
        assert_ne!(hash(&|h| h.write(&[1; 9])), hash(&|h| h.write(&[1; 8])));
    }

    // Errors.

    #[test]
    fn duplicate_nets_are_rejected() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let nets = [net(4, &[0, 1]), net(9, &[0, 1]), net(4, &[0, 1])];
        let error = route(&grid, &nets, options()).unwrap_err();
        assert_eq!(error, EngineError::DuplicateNet { net: NetId(4) });
        assert_eq!(error.to_string(), "net 4 is listed twice");
    }

    #[test]
    fn no_nets_is_a_clean_success() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let outcome = route_checked(&grid, &[], options());
        assert!(outcome.is_success());
        assert_eq!(outcome.stop, StopReason::Converged);
    }

    #[test]
    fn net_ids_need_not_be_dense_or_in_order() {
        let grid = Grid::new(3, 2, 1).unwrap();
        let nets = [net(900, &[3, 5]), net(7, &[0, 2])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
    }

    #[test]
    fn engine_debug_is_a_summary() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let engine = Engine::new(&grid, &[], options()).unwrap();
        let text = format!("{engine:?}");
        assert!(text.contains("nets: 0"), "{text}");
    }

    // Spaces that are not dense.

    /// A lazy grid whose nodes and resources are coordinate pairs, whose resources do not map to
    /// any dense index, and whose search slots are interned.
    struct Lazy {
        size: i32,
    }

    impl RoutingSpace for Lazy {
        type Node = (i32, i32);
        type Resource = (i32, i32);
        type Slots = InternedSlots<(i32, i32)>;

        fn new_slots(&self) -> Self::Slots {
            InternedSlots::new()
        }

        fn expand(&self, _net: NetId, (x, y): (i32, i32), out: &mut Vec<Step<(i32, i32)>>) {
            for to in [(x + 1, y), (x, y + 1), (x - 1, y), (x, y - 1)] {
                if (0..self.size).contains(&to.0) && (0..self.size).contains(&to.1) {
                    out.push(Step {
                        to,
                        cost: Cost::ONE,
                    });
                }
            }
        }

        fn lower_bound(&self, _net: NetId, from: (i32, i32), to: (i32, i32)) -> Cost {
            Cost::from(from.0.abs_diff(to.0) + from.1.abs_diff(to.1))
        }

        fn resources(&self, _: NetId, _: (i32, i32), to: (i32, i32), out: &mut Vec<(i32, i32)>) {
            out.push(to);
        }

        fn capacity(&self, _resource: (i32, i32)) -> u32 {
            1
        }
    }

    #[test]
    fn works_on_generated_nodes_and_resources() {
        let space = Lazy { size: 6 };
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![Terminal::at((0, 1)), Terminal::at((5, 1))],
            },
            Net {
                id: NetId(1),
                terminals: vec![Terminal::at((1, 0)), Terminal::at((3, 3))],
            },
        ];
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
    }

    // Determinism.

    /// A dense problem, for determinism and stress: every row's left end to a shuffled row's
    /// right end, on two layers.
    fn permutation_problem(size: u32, layers: u32, seed: u64) -> (Grid, Vec<Net<NodeId>>) {
        let grid = Grid::new(size, size, layers).unwrap();
        let rows = permutation(size, seed);
        let nets = rows
            .iter()
            .enumerate()
            .map(|(y, &to)| {
                let from = grid.node_at(0, y as u32, 0).unwrap().0;
                let to = grid.node_at(size - 1, to, 0).unwrap().0;
                net(y as u32, &[from, to])
            })
            .collect();
        (grid, nets)
    }

    #[test]
    fn repeated_runs_are_identical() {
        let (grid, nets) = permutation_problem(12, 2, 3);
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        while engine.step().is_some() {}
        let first = engine.outcome();
        let reports = engine.reports().to_vec();
        assert!(reports.len() > 1, "this should take some negotiation");
        for _ in 0..3 {
            let mut again = Engine::new(&grid, &nets, options()).unwrap();
            while again.step().is_some() {}
            assert_eq!(again.outcome(), first);
            assert_eq!(again.reports(), reports);
        }
    }

    #[test]
    fn shuffled_runs_repeat_for_a_seed_and_differ_between_seeds() {
        let (grid, nets) = permutation_problem(12, 2, 3);
        let seeded = |seed| {
            let shuffled = EngineOptions {
                shuffle_seed: Some(seed),
                ..options()
            };
            route_checked(&grid, &nets, shuffled)
        };
        let first = seeded(1);
        assert_eq!(seeded(1), first);
        assert!(first.is_success());
        let differs = (2..10).any(|seed| seeded(seed) != first);
        assert!(differs);
    }

    #[test]
    fn input_order_is_the_only_order_that_matters() {
        // Hash seeds differ between hash maps, which must not show. Each search map is new here.
        let (grid, nets) = permutation_problem(10, 2, 8);
        let expected = route_checked(&grid, &nets, options());
        let spaces: Vec<Grid> = (0..4).map(|_| grid.clone()).collect();
        for space in &spaces {
            assert_eq!(route_checked(space, &nets, options()), expected);
        }
    }

    /// A regression value and not a reference one: it came from this implementation, and it
    /// pins the combined behaviour of the search, the prices and the schedule to the bit. Costs
    /// only ever go through `f64` addition and multiplication in a fixed order, which IEEE 754
    /// defines exactly, so it must come out the same on Linux, macOS and Windows.
    ///
    /// A deliberate change to routing will change it. Update it then, and not otherwise.
    #[test]
    fn routes_match_their_regression_value() {
        let (grid, nets) = permutation_problem(16, 2, 42);
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(hash_outcome(&outcome), 13615641185186696971);
    }

    // Stress. These print how they converged, which `--nocapture` shows.

    /// Runs a problem to the end, printing its progress, and requires a legal result.
    fn stress<S: RoutingSpace>(space: &S, nets: &[Net<S::Node>]) -> Outcome<S::Node, S::Resource> {
        let mut engine = Engine::new(space, nets, options()).unwrap();
        while let Some(report) = engine.step() {
            println!(
                "iteration {}: rerouted {}, expanded {}, overused {}, cost {}",
                report.iteration,
                report.nets_rerouted,
                report.nodes_expanded,
                report.total_overuse,
                report.total_cost
            );
        }
        let outcome = engine.outcome();
        assert_legal(space, nets, &outcome);
        outcome
    }

    /// As dense as this kind of problem gets: every cell of both side columns is a terminal, and
    /// with two layers almost all of the crossings have to be negotiated.
    #[test]
    fn dense_grid_stress_converges() {
        let (grid, nets) = permutation_problem(32, 2, 7);
        let outcome = stress(&grid, &nets);
        assert!(outcome.routes.stats.iterations > 3);
    }

    /// Larger, and with nets of up to four terminals, at a density that takes negotiating.
    #[test]
    fn dense_multi_terminal_grid_stress_converges() {
        let grid = Grid::new(48, 48, 2).unwrap();
        let mut rng = crate::rng::Rng::new(11);
        let mut cells = permutation(48 * 48, 12);
        let nets: Vec<_> = (0..40)
            .map(|id| {
                let count = 2 + rng.below(3) as usize;
                let at = cells.split_off(cells.len() - count);
                net(id, &at)
            })
            .collect();
        let outcome = stress(&grid, &nets);
        assert!(outcome.routes.stats.iterations > 1);
    }

    #[test]
    fn large_permutation_stress_converges() {
        let (grid, nets) = permutation_problem(48, 2, 1);
        let outcome = stress(&grid, &nets);
        assert!(outcome.routes.stats.iterations > 3);
    }

    /// An island-style FPGA fabric: a square of tiles, each with one pin and a number of tracks
    /// in each direction. The width of the channels is the one that matters here.
    struct Fabric {
        size: u32,
        tracks: u32,
    }

    impl Fabric {
        fn per_tile(&self) -> u32 {
            2 * self.tracks + 1
        }

        fn pin(&self, x: u32, y: u32) -> u32 {
            (x + self.size * y) * self.per_tile()
        }

        fn wire(&self, x: u32, y: u32, vertical: bool, track: u32) -> u32 {
            self.pin(x, y) + 1 + track * 2 + u32::from(vertical)
        }

        fn build(&self) -> impl RoutingSpace<Node = NodeId, Resource = NodeId> + use<> {
            let mut builder = GraphBuilder::new(self.size * self.size * self.per_tile());
            let mut both_ways = |a: u32, b: u32| {
                builder.add_edge(NodeId(a), NodeId(b), Cost::ONE).unwrap();
                builder.add_edge(NodeId(b), NodeId(a), Cost::ONE).unwrap();
            };
            for y in 0..self.size {
                for x in 0..self.size {
                    for track in 0..self.tracks {
                        let horizontal = self.wire(x, y, false, track);
                        let vertical = self.wire(x, y, true, track);
                        both_ways(self.pin(x, y), horizontal);
                        both_ways(self.pin(x, y), vertical);
                        both_ways(horizontal, vertical);
                        both_ways(horizontal, self.wire(x, y, true, (track + 1) % self.tracks));
                        if x + 1 < self.size {
                            both_ways(horizontal, self.wire(x + 1, y, false, track));
                        }
                        if y + 1 < self.size {
                            both_ways(vertical, self.wire(x, y + 1, true, track));
                        }
                    }
                }
            }
            // Each step costs 1 and moves at most one tile.
            let (size, per_tile) = (self.size, self.per_tile());
            builder
                .build()
                .with_lower_bound(move |from: NodeId, to: NodeId| {
                    let (from, to) = (from.0 / per_tile, to.0 / per_tile);
                    Cost::from(
                        (from % size).abs_diff(to % size) + (from / size).abs_diff(to / size),
                    )
                })
        }

        /// Nets of two to four terminals, on pins that no two nets share.
        fn nets(&self, count: u32, seed: u64) -> Vec<Net<NodeId>> {
            let mut rng = crate::rng::Rng::new(seed);
            let mut tiles: Vec<u32> = (0..self.size * self.size).collect();
            rng.shuffle(&mut tiles);
            let mut tiles = tiles.into_iter();
            (0..count)
                .map(|id| {
                    let terminals = 2 + rng.below(3);
                    let pins: Vec<u32> = (0..terminals)
                        .filter_map(|_| tiles.next())
                        .map(|tile| self.pin(tile % self.size, tile / self.size))
                        .collect();
                    net(id, &pins)
                })
                .collect()
        }
    }

    /// Five tracks is the narrowest width that this problem routes on.
    #[test]
    fn fabric_at_minimum_channel_width_converges() {
        let fabric = Fabric {
            size: 16,
            tracks: 5,
        };
        let (space, nets) = (fabric.build(), fabric.nets(80, 2));
        let outcome = stress(&space, &nets);
        assert!(outcome.routes.stats.iterations >= 2);
    }

    /// Far too few tracks, and the result has to say that it did not work.
    #[test]
    fn fabric_below_minimum_channel_width_reports_failure() {
        let fabric = Fabric {
            size: 12,
            tracks: 1,
        };
        let (space, nets) = (fabric.build(), fabric.nets(40, 2));
        let limited = EngineOptions {
            max_iterations: 15,
            ..options()
        };
        let outcome = route_checked(&space, &nets, limited);
        assert!(!outcome.is_success());
        assert_eq!(outcome.stop, StopReason::IterationLimit);
        assert!(!outcome.overused.is_empty());
    }

    // Properties.

    /// Random terminals over `0..nodes`, as `(node, access cost)`, for up to `nets` nets.
    fn any_nets(nodes: u32, nets: usize) -> impl Strategy<Value = Vec<Net<NodeId>>> {
        let point = (0..nodes, 0..4u32);
        let terminal = prop::collection::vec(point, 1..3).prop_map(|points| terminal(&points));
        prop::collection::vec(prop::collection::vec(terminal, 0..5), 0..nets).prop_map(|list| {
            list.into_iter()
                .enumerate()
                .map(|(id, terminals)| Net {
                    id: NetId(id as u32 * 3 + 1),
                    terminals,
                })
                .collect()
        })
    }

    /// A small grid with random blocks and capacities.
    fn any_grid_problem() -> impl Strategy<Value = (Grid, Vec<Net<NodeId>>)> {
        (2u32..7, 2u32..7, 1u32..3).prop_flat_map(|(width, height, layers)| {
            let cells = (width * height * layers) as usize;
            (
                Just((width, height, layers)),
                prop::collection::vec((prop::bool::weighted(0.1), 0u32..4), cells),
                any_nets(width * height * layers, 8),
            )
                .prop_map(|((width, height, layers), cells, nets)| {
                    let mut grid = Grid::new(width, height, layers).unwrap();
                    for (id, &(blocked, capacity)) in cells.iter().enumerate() {
                        let (x, y, layer) = grid.position(NodeId(id as u32)).unwrap();
                        grid.set_blocked(x, y, layer, blocked).unwrap();
                        // Mostly the default of 1, with some zero and some larger.
                        let capacity = [1, 1, 0, 2][capacity as usize];
                        grid.set_capacity(x, y, layer, capacity).unwrap();
                    }
                    (grid, nets)
                })
        })
    }

    /// A small directed graph with random costs, including free steps, and random capacities.
    fn any_graph_problem() -> impl Strategy<Value = (Graph, Vec<Net<NodeId>>)> {
        (2u32..20).prop_flat_map(|nodes| {
            (
                prop::collection::vec((0..nodes, 0..nodes, 0..4u32), 0..60),
                prop::collection::vec(0u32..4, nodes as usize),
                any_nets(nodes, 8),
            )
                .prop_map(move |(edges, capacities, nets)| {
                    let mut builder = GraphBuilder::new(nodes);
                    for (from, to, cost) in edges {
                        builder
                            .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                            .unwrap();
                    }
                    for (node, &capacity) in capacities.iter().enumerate() {
                        builder
                            .set_capacity(NodeId(node as u32), [1, 1, 0, 2][capacity as usize])
                            .unwrap();
                    }
                    (builder.build(), nets)
                })
        })
    }

    /// The checks that any problem has to pass, on any space.
    fn check_problem<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
    ) -> Result<(), TestCaseError> {
        let limited = EngineOptions {
            max_iterations: 12,
            ..options()
        };
        let outcome = route(space, nets, limited).unwrap();

        // Whatever it claims, it must be the truth.
        let honest = check_honest(space, nets, &outcome);
        prop_assert!(honest.is_ok(), "{:?}", honest);
        if outcome.is_success() {
            let legal = check_legal(space, nets, &outcome.routes);
            prop_assert!(legal.is_ok(), "{:?}", legal);
        }

        // A net is routed exactly when a path exists, as no budget was set.
        for net in nets {
            let can = routable(space, net);
            let result = outcome.routes.outcome(net.id);
            match result {
                Some(NetOutcome::Routed(_)) => {
                    prop_assert!(can, "{:?} routed without a path", net.id)
                }
                Some(NetOutcome::Unrouted(UnroutedReason::NoPath)) => {
                    prop_assert!(!can, "{:?} reported as no path, but has one", net.id);
                }
                other => prop_assert!(false, "{:?} ended up as {:?}", net.id, other),
            }
        }

        // Stepping and running agree, as do repeated runs.
        let mut engine = Engine::new(space, nets, limited).unwrap();
        while engine.step().is_some() {}
        prop_assert_eq!(engine.outcome(), outcome);
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn grid_problems_get_honest_results((grid, nets) in any_grid_problem()) {
            check_problem(&grid, &nets)?;
        }

        #[test]
        fn graph_problems_get_honest_results((graph, nets) in any_graph_problem()) {
            check_problem(&graph, &nets)?;
        }

        #[test]
        fn partial_budgets_give_honest_results(
            (grid, nets) in any_grid_problem(),
            budget in 0u64..200,
        ) {
            let limited = EngineOptions {
                max_iterations: 8,
                max_expansions: budget,
                ..options()
            };
            let outcome = route(&grid, &nets, limited).unwrap();
            let honest = check_honest(&grid, &nets, &outcome);
            prop_assert!(honest.is_ok(), "{:?}", honest);
            prop_assert!(outcome.routes.stats.nodes_expanded <= budget);
            for (id, reason) in outcome.routes.unrouted() {
                let net = nets.iter().find(|net| net.id == id).unwrap();
                if reason == UnroutedReason::NoPath {
                    prop_assert!(!routable(&grid, net));
                } else {
                    prop_assert_eq!(reason, UnroutedReason::BudgetExhausted);
                }
            }
        }
    }
}
