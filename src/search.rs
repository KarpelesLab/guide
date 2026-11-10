//! Single-net shortest-path search over any [`RoutingSpace`].
//!
//! [`Search`] runs a multi-source, multi-target A\* and keeps its state between runs, so
//! that routing many nets does not allocate for each one. The heuristic is the minimum
//! of [`RoutingSpace::lower_bound`] over the targets. A space whose lower bound is
//! [`Cost::ZERO`] gets Dijkstra's algorithm, with no separate implementation.
//!
//! # Determinism
//!
//! The order nodes leave the queue is a total order that depends only on the space's
//! answers and on the order sources are passed in. Queue entries are compared by:
//!
//! 1. Lowest `f = g + h`, using [`Cost::key`].
//! 2. Highest `g`. Among equally promising entries this prefers the one that has done
//!    the most real work, and since a target's `h` is zero, a target is taken before
//!    anything that could only tie with it. Both avoid needless expansions.
//! 3. Earliest insertion. Sources are inserted in the order given and steps in the order
//!    [`RoutingSpace::expand`] returned them, so equal paths are resolved by that order.
//!
//! A node's cost and parent only change on a strict improvement, so the first of several
//! equally cheap ways to reach a node is the one that is kept. Slot numbers, addresses
//! and hash order play no part. Search state does not leak between runs: a reused
//! [`Search`] and a new one give identical paths, costs and statistics.
//!
//! # Optimality
//!
//! Let the *cost* of a path be the left-to-right sum of its starting cost and step costs,
//! as computed by [`Cost`] addition with its rounding. The search stops when a target
//! leaves the queue, and it re-opens any node it finds a cheaper way to, so an admissible
//! lower bound does not also have to be consistent. Three levels of guarantee follow:
//!
//! - **Exact arithmetic.** If the lower bound is admissible and no sum involved rounds
//!   (integer costs with totals up to `2^53`, for example), the result is optimal.
//! - **Zero lower bound.** With [`Cost::ZERO`] everywhere, the result is optimal even when
//!   sums round: it has the minimum cost as defined above. This works because rounded
//!   addition is monotonic, so `a <= b` implies `a + c <= b + c`, and never makes a total
//!   smaller.
//! - **Rounding with a non-zero lower bound.** Addition is not associative once it rounds,
//!   so `g + h` may exceed the cost of the path it estimates, and a target may be taken
//!   slightly too early. The error is bounded, however. If `S` is the exact, unrounded
//!   sum of the cheapest path, `k` is that path's number of steps, and the lower bound
//!   never exceeds the exact remaining sum, then [`Found::cost`] is at most
//!   `S * (1 + 2^-53)^(k + 1)`. That is a relative error of about `1e-13` for a path of
//!   a thousand steps. A bound that itself carries rounding error (a hop count times
//!   `0.1`, say) adds its own relative error on top.
//!
//! An inadmissible bound voids these guarantees, but the search still terminates and
//! returns a valid path. So does a [weight](Search::set_bound_weight) above 1 on the bound,
//! which is weighted A\*: with an admissible bound, the path found then costs at most that
//! many times the cheapest.
//!
//! # A bound of the maximum
//!
//! A node whose heuristic is [`Cost::MAX`] can only leave the queue once every entry left has
//! an `f` of [`Cost::MAX`], so its offers are held back until then, and made in their order,
//! with the insertion order they would have had. Nothing a search finds changes, nor any
//! statistic but [`SearchStats::pushes`], which only counts what was queued. A space can use
//! this to say that a node has no way to a target, and a search that ends first never queues
//! that node at all.
//!
//! [`Found::cost`] is the cost of [`Found::path`] whenever no sum involved rounds or
//! saturates, and always with a zero lower bound. Otherwise the path's own cost can be
//! lower: a node may improve by so little that its `f` does not change, which lets a
//! target reached by way of its old cost come first. The reported cost is never below
//! the cost of the path.
//!
//! Costs that saturate at [`Cost::MAX`] all compare equal, so among paths too expensive
//! to represent the search returns one of them, chosen by the same deterministic rules.
//!
//! # Nodes outside the space
//!
//! Sources and targets that the space does not [contain](RoutingSpace::contains) are
//! ignored, as if they had not been listed, before anything is done with them. A slot
//! mapping might turn such a node into a huge index, so it never gets to see one.
//!
//! # Limits
//!
//! [`Search::find_path`] runs until it reaches a target or runs out of nodes. On an unbounded
//! space with an unreachable target it does not return, so such spaces have to bound
//! themselves in [`expand`](RoutingSpace::expand), or be searched through
//! [`Search::find_path_with`], which takes [`SearchLimits`] and reports running out of
//! them apart from finding nothing. An inconsistent bound can also cause many
//! re-expansions, exponentially many in contrived worst cases.
//!
//! # Target costs
//!
//! [`Search::find_path_with`] takes a [`Target`] with a cost per target, as if each target
//! had one more step of that cost into a shared final node. That is how the search treats
//! it. A target that leaves the queue queues its own finish at `g + cost` and is expanded
//! like any other node, because a dearer target may lie on the way to a cheaper one. The
//! search ends when a finish leaves the queue. The heuristic of a non-target node adds each
//! target's cost to the bound for that target, and stays admissible, while a target's own
//! heuristic stays zero, since a route may also go on from there to a target with a lower
//! cost. Everything above still applies, with paths costed up to and including the finish.

use core::fmt;
use core::mem;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cost::Cost;
use crate::id::NetId;
use crate::space::{NodeSlots, RoutingSpace, Step};

/// Marks a node with no parent in its slot's `parent` field: the path starts there.
const NO_PARENT: usize = usize::MAX;

/// Marks a node in its slot's `parent` field whose heuristic is [`Cost::MAX`], and whose
/// offers are held back, so that it has no cost or parent yet. See [`Search::deferred`].
const DEFERRED: usize = usize::MAX - 1;

/// Set in a [`HeapEntry`]'s `slot` when the entry is a target's finish rather than the node
/// itself. Slots index arrays in memory, so they never get this high.
const FINISH_BIT: usize = 1 << (usize::BITS - 1);

/// How many expansions pass between looks at a cancel flag. A flag is cheap to read but
/// lives on a cache line that another thread may write, and a search gets through this
/// many expansions in well under a millisecond in a release build.
const CANCEL_CHECK_INTERVAL: u64 = 1024;

/// A node a search may start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source<N> {
    /// The node to start from.
    pub node: N,
    /// The cost already paid on arrival at the node, such as a pad-access cost.
    pub cost: Cost,
}

impl<N> Source<N> {
    /// Creates a source that costs nothing to start from.
    #[inline]
    pub const fn new(node: N) -> Self {
        Source {
            node,
            cost: Cost::ZERO,
        }
    }

    /// Creates a source with a starting cost.
    #[inline]
    pub const fn with_cost(node: N, cost: Cost) -> Self {
        Source { node, cost }
    }
}

/// A node a search may finish at, for [`Search::find_path_with`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target<N> {
    /// The node to finish at.
    pub node: N,
    /// The cost of finishing here, on top of the cost of getting here, such as a
    /// pad-access cost.
    pub cost: Cost,
}

impl<N> Target<N> {
    /// Creates a target that costs nothing to finish at.
    #[inline]
    pub const fn new(node: N) -> Self {
        Target {
            node,
            cost: Cost::ZERO,
        }
    }

    /// Creates a target with a finishing cost.
    #[inline]
    pub const fn with_cost(node: N, cost: Cost) -> Self {
        Target { node, cost }
    }
}

/// What may stop a [`Search::find_path_with`] before it reaches a target.
#[derive(Clone, Copy, Debug)]
pub struct SearchLimits<'a> {
    /// How many expansions the search may make, re-expansions included. A search that can
    /// finish without another expansion still does, so zero allows a source that is also
    /// a target of no cost.
    pub max_expansions: u64,
    /// A flag that stops the search once set. It is read before the first expansion and
    /// about every thousand after that, so a search that expands nothing never reads it.
    pub cancel: Option<&'a AtomicBool>,
}

impl SearchLimits<'_> {
    /// No limits, which is how [`Search::find_path`] runs.
    pub const NONE: SearchLimits<'static> = SearchLimits {
        max_expansions: u64::MAX,
        cancel: None,
    };
}

impl Default for SearchLimits<'_> {
    #[inline]
    fn default() -> Self {
        SearchLimits::NONE
    }
}

/// How a [`Search::find_path_with`] ended.
///
/// It borrows the search's buffers when a path was found, like [`Found`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchOutcome<'a, N> {
    /// The cheapest path.
    Found(Found<'a, N>),
    /// Every node reachable from the sources was explored, so no target can be reached
    /// at any budget. This includes `sources` or `targets` being empty, or holding only
    /// nodes outside the space.
    Unreachable,
    /// [`SearchLimits::max_expansions`] ran out first. A target may or may not be reachable.
    LimitReached,
    /// [`SearchLimits::cancel`] was set first. A target may or may not be reachable.
    Cancelled,
}

/// A path found by [`Search::find_path`] or [`Search::find_path_with`].
///
/// It borrows the search's buffers, so copy out what you need to keep before starting
/// the next search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found<'a, N> {
    /// The nodes in order, from a source to a target. A source that is also the cheapest
    /// target gives a path of just that node.
    pub path: &'a [N],
    /// The total cost, including the starting cost of the source and the cost of the target.
    pub cost: Cost,
}

/// Counters for the most recent search, which are reset when the next one starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// How many times a node was expanded, counting re-expansions of re-opened nodes.
    pub expanded: u64,
    /// How many entries were pushed onto the queue, sources included.
    pub pushes: u64,
    /// How many queue entries were discarded because a cheaper one had replaced them.
    pub stale_pops: u64,
}

/// Search state for one slot.
///
/// The fields are all touched together when a node is reached, so keeping them in one
/// array means one cache miss rather than one per field. The node itself lives elsewhere
/// because it is only needed on expansion.
#[derive(Clone, Copy)]
struct SlotState {
    /// The cheapest known cost to this node. Valid if `reached` is current.
    g: Cost,
    /// The cached heuristic, since evaluating it costs one bound per target. Valid if
    /// `reached` is current.
    h: Cost,
    /// The slot this node was reached from. Valid if `reached` is current.
    parent: usize,
    /// The generation this node was last reached in. `g` cannot serve as a marker,
    /// because every value including [`Cost::MAX`] is a legitimate cost.
    reached: u32,
    /// The generation this node was last a target in.
    target: u32,
}

impl SlotState {
    /// A slot untouched by any search. Generations start from one, so zero never matches.
    const UNSEEN: SlotState = SlotState {
        g: Cost::ZERO,
        h: Cost::ZERO,
        parent: NO_PARENT,
        reached: 0,
        target: 0,
    };
}

/// A queue entry, with its comparison fields reduced to integers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeapEntry {
    /// [`Cost::key`] of `g + h`.
    f: u64,
    /// The bitwise complement of [`Cost::key`] of `g`, so that ascending order on this
    /// field prefers larger `g`. It also identifies stale entries: a node's `g` only ever
    /// falls, so an entry whose `g` is not the current one has been superseded.
    inverted_g: u64,
    /// Insertion order, unique within a search.
    sequence: u64,
    /// The slot this entry is for.
    slot: usize,
}

impl HeapEntry {
    /// Returns whether this entry leaves the queue before `other`.
    #[inline]
    fn precedes(&self, other: &HeapEntry) -> bool {
        if self.f != other.f {
            self.f < other.f
        } else if self.inverted_g != other.inverted_g {
            self.inverted_g < other.inverted_g
        } else {
            self.sequence < other.sequence
        }
    }
}

/// How many children each heap node has.
const HEAP_ARITY: usize = 4;

/// A 4-ary min-heap of [`HeapEntry`].
///
/// A wider node makes the tree shallower, which saves more cache misses on the way down
/// than the extra comparisons per level cost. Whole searches over grids of up to 9 million
/// nodes ran about 10% faster than with `std::collections::BinaryHeap`, ordered the same
/// way. That is not dramatic, but this is the hottest loop in the crate and the heap
/// is small.
#[derive(Default)]
struct QuadHeap {
    entries: Vec<HeapEntry>,
}

impl QuadHeap {
    #[inline]
    fn clear(&mut self) {
        self.entries.clear();
    }

    #[inline]
    fn push(&mut self, entry: HeapEntry) {
        let mut hole = self.entries.len();
        self.entries.push(entry);
        while hole > 0 {
            let parent = (hole - 1) / HEAP_ARITY;
            if !entry.precedes(&self.entries[parent]) {
                break;
            }
            self.entries[hole] = self.entries[parent];
            hole = parent;
        }
        self.entries[hole] = entry;
    }

    /// Returns the entry that would leave the queue next.
    #[inline]
    fn peek(&self) -> Option<&HeapEntry> {
        self.entries.first()
    }

    #[inline]
    fn pop(&mut self) -> Option<HeapEntry> {
        let last = self.entries.pop()?;
        let Some(&top) = self.entries.first() else {
            return Some(last);
        };

        let len = self.entries.len();
        let mut hole = 0;
        loop {
            let first_child = hole * HEAP_ARITY + 1;
            if first_child >= len {
                break;
            }
            let children = &self.entries[first_child..len.min(first_child + HEAP_ARITY)];

            let mut best = 0;
            for (index, child) in children.iter().enumerate().skip(1) {
                if child.precedes(&children[best]) {
                    best = index;
                }
            }
            let best_child = children[best];
            if !best_child.precedes(&last) {
                break;
            }
            self.entries[hole] = best_child;
            hole = first_child + best;
        }
        self.entries[hole] = last;
        Some(top)
    }
}

/// Reusable state for shortest-path searches over one space.
///
/// Buffers grow to fit the largest search seen and are then reused: only the buffers'
/// growth allocates, along with whatever the space does itself in
/// [`expand`](RoutingSpace::expand) and [`NodeSlots::slot`]. Between searches, state is
/// discarded in constant time by advancing a generation counter.
///
/// See the [module documentation](self) for the determinism and optimality guarantees.
///
/// A `Search` holds the slot mapping from [`RoutingSpace::new_slots`]. It should only be
/// used with the space it was created from, or one whose slot mapping is interchangeable
/// with it.
pub struct Search<S: RoutingSpace> {
    slots: S::Slots,
    states: Vec<SlotState>,
    /// The node for each slot, valid where the slot's `reached` is current. Spaces only
    /// offer the node-to-slot direction of the mapping, so the reverse is kept here.
    nodes: Vec<S::Node>,
    /// The current generation, which starts from one.
    generation: u32,
    heap: QuadHeap,
    next_sequence: u64,
    /// The distinct targets of the current search, with the lowest cost given for each.
    targets: Vec<Target<S::Node>>,
    /// Where each slot's target is in `targets`. Valid if the slot's `target` is current.
    /// Apart from [`SlotState`] because it is only read when a target leaves the queue.
    target_positions: Vec<usize>,
    /// The expansion buffer.
    steps: Vec<Step<S::Node>>,
    /// The path of the most recent search.
    path: Vec<S::Node>,
    stats: SearchStats,
    /// What the heuristic is multiplied by: see [`Search::set_bound_weight`].
    bound_weight: f64,
    /// Offers of nodes whose heuristic is [`Cost::MAX`], with the sequence number each would
    /// have been queued with, in the order made.
    ///
    /// Such a node's entries have an `f` of [`Cost::MAX`], and leave the queue after every
    /// entry with a lower `f`, so queueing them can wait until the queue has nothing lower.
    /// Then they are offered again in order, and queued with the sequence numbers they would
    /// have had. Every entry leaves the queue in the same order as without this, so nothing
    /// a search finds changes. But a search that ends first, as nearly all do, never queues
    /// them. A space can give [`Cost::MAX`] to say that there is no way to a target, as
    /// `fpga::MapLookahead` does for the pins of other blocks, and those cost very little.
    deferred: Vec<(usize, Cost, usize, u64)>,
    /// Whether the offers held back have been made, after which none are held back.
    flushed: bool,
    /// Whether the next search may keep heuristics from this one: see
    /// [`Search::reuse_bounds`].
    reuse_next: bool,
    /// The targets of the previous search, while the current one decides what it can reuse.
    previous_targets: Vec<Target<S::Node>>,
    /// The targets of the previous search that the current one does not have, when it
    /// reuses heuristics, and otherwise empty.
    removed: Vec<Target<S::Node>>,
    /// For each target of the current search, where it was among the previous one's, while
    /// the current one decides what it can reuse.
    was_at: Vec<usize>,
    /// For each target of the previous search, whether the current one has it too.
    kept: Vec<bool>,
    /// Whether the current search reuses the heuristics of the previous one.
    reusing: bool,
}

impl<S: RoutingSpace> fmt::Debug for Search<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Search")
            .field("slots_allocated", &self.states.len())
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl<S: RoutingSpace> Search<S> {
    /// Creates a search for `space`, with empty buffers.
    pub fn new(space: &S) -> Self {
        Search {
            slots: space.new_slots(),
            states: Vec::new(),
            nodes: Vec::new(),
            generation: 0,
            heap: QuadHeap::default(),
            next_sequence: 0,
            targets: Vec::new(),
            target_positions: Vec::new(),
            steps: Vec::new(),
            path: Vec::new(),
            stats: SearchStats::default(),
            bound_weight: 1.0,
            deferred: Vec::new(),
            flushed: false,
            reuse_next: false,
            previous_targets: Vec::new(),
            removed: Vec::new(),
            was_at: Vec::new(),
            kept: Vec::new(),
            reusing: false,
        }
    }

    /// Lets the next search keep the heuristic of each node that this one reached, where it
    /// is sure to come out the same, which it then does not ask the space for again.
    ///
    /// The caller promises that the next search is for the same net, through a space with the
    /// same [`lower_bound`](RoutingSpace::lower_bound), at the same
    /// [weight](Search::set_bound_weight). The search checks the rest: that its targets are
    /// some of this one's, with the same costs. Then a node's heuristic, the lowest bound over
    /// the targets, can only have changed if it came from a target that is gone, which only
    /// those targets are asked about. Nothing a search finds changes.
    ///
    /// This is for a tree growing one connection at a time, whose every node is a source of
    /// each search, towards every terminal it has still to reach. Without it a net of `n`
    /// terminals asks for bounds on the order of `n` cubed times.
    pub(crate) fn reuse_bounds(&mut self) {
        self.reuse_next = true;
    }

    /// Multiplies the heuristic by `weight` in the searches that follow, which is weighted
    /// A\*. The default is 1, the plain bound.
    ///
    /// Above 1, a search goes more directly at its target and expands fewer nodes, but the
    /// heuristic overestimates by up to that factor, so the optimality guarantees of the
    /// [module documentation](self) no longer hold: the path found costs at most `weight` times
    /// the cheapest, given an admissible bound. VPR's router does the same with its
    /// `--astar_fac`, by default 1.2. A weight that is not finite or below 0 is taken as 1, and
    /// at 0 the search is Dijkstra's algorithm.
    pub fn set_bound_weight(&mut self, weight: f64) {
        self.bound_weight = if weight.is_finite() && weight >= 0.0 {
            weight
        } else {
            1.0
        };
    }

    /// Returns the counters for the most recent search.
    #[inline]
    pub fn stats(&self) -> SearchStats {
        self.stats
    }

    /// Finds the cheapest path for `net` from any of `sources` to any of `targets`.
    ///
    /// Returns `None` if no target can be reached, which includes `sources` or `targets`
    /// being empty. Nodes that the space does not [contain](RoutingSpace::contains) are
    /// ignored, in either list, so a node outside the space cannot be a path of its own.
    ///
    /// A node may be listed more than once. Among duplicate sources the lowest starting
    /// cost wins, and duplicate targets make no difference. A source that is also a
    /// target can be a path of one node, but a different source can still beat it if its
    /// total is lower.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::cost::Cost;
    /// use guide::id::{NetId, NodeId};
    /// use guide::search::{Search, Source};
    /// use guide::space::{DenseSlots, RoutingSpace, Step};
    ///
    /// /// A row of cells, each one unit of cost from its neighbours.
    /// struct Row {
    ///     len: u32,
    /// }
    ///
    /// impl RoutingSpace for Row {
    ///     type Node = NodeId;
    ///     type Resource = NodeId;
    ///     type Slots = DenseSlots<NodeId>;
    ///     type ResourceSlots = DenseSlots<NodeId>;
    ///
    ///     fn new_slots(&self) -> Self::Slots {
    ///         DenseSlots::new()
    ///     }
    ///
    ///     fn new_resource_slots(&self) -> Self::ResourceSlots {
    ///         DenseSlots::new()
    ///     }
    ///
    ///     fn contains(&self, node: NodeId) -> bool {
    ///         node.0 < self.len
    ///     }
    ///
    ///     fn expand(&self, _net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
    ///         let neighbours = [from.0.checked_sub(1), from.0.checked_add(1)];
    ///         for to in neighbours.into_iter().flatten().filter(|&to| to < self.len) {
    ///             out.push(Step { to: NodeId(to), cost: Cost::ONE });
    ///         }
    ///     }
    ///
    ///     fn lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> Cost {
    ///         Cost::from(from.0.abs_diff(to.0))
    ///     }
    ///
    ///     fn resources(&self, _: NetId, _: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
    ///         out.push(to);
    ///     }
    ///
    ///     fn capacity(&self, _resource: NodeId) -> u32 {
    ///         1
    ///     }
    /// }
    ///
    /// let row = Row { len: 8 };
    /// let mut search = Search::new(&row);
    ///
    /// // Start from cell 1 for free, or from cell 6 for a price; finish at cell 4.
    /// let sources = [
    ///     Source::new(NodeId(1)),
    ///     Source::with_cost(NodeId(6), Cost::from(5)),
    /// ];
    /// let found = search
    ///     .find_path(&row, NetId(0), sources, [NodeId(4)])
    ///     .unwrap();
    /// assert_eq!(found.path, [NodeId(1), NodeId(2), NodeId(3), NodeId(4)]);
    /// assert_eq!(found.cost, Cost::from(3));
    ///
    /// // The heuristic went straight there, leaving cells 0 and 5 unexpanded.
    /// assert_eq!(search.stats().expanded, 3);
    /// ```
    pub fn find_path(
        &mut self,
        space: &S,
        net: NetId,
        sources: impl IntoIterator<Item = Source<S::Node>>,
        targets: impl IntoIterator<Item = S::Node>,
    ) -> Option<Found<'_, S::Node>> {
        let targets = targets.into_iter().map(Target::new);
        match self.find_path_with(space, net, sources, targets, SearchLimits::NONE) {
            SearchOutcome::Found(found) => Some(found),
            _ => None,
        }
    }

    /// Like [`find_path`](Search::find_path), with target costs and limits, and with a result
    /// that tells running out of limits apart from there being no path.
    ///
    /// Among duplicate targets the lowest cost wins. The `space` need not be the one the
    /// search was created from, as long as its nodes map to slots in the same way, so that
    /// a wrapper around a space can be searched with a `Search` that outlives the wrapper.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::cost::Cost;
    /// use guide::id::{NetId, NodeId};
    /// use guide::search::{Search, SearchLimits, SearchOutcome, Source, Target};
    /// use guide::space::grid::Grid;
    ///
    /// let grid = Grid::new(8, 1, 1)?;
    /// let mut search = Search::new(&grid);
    /// let source = [Source::new(NodeId(0))];
    ///
    /// // Node 2 is closer, but costs more to finish at.
    /// let targets = [
    ///     Target::with_cost(NodeId(2), Cost::from(9)),
    ///     Target::new(NodeId(5)),
    /// ];
    /// let outcome =
    ///     search.find_path_with(&grid, NetId(0), source, targets, SearchLimits::NONE);
    /// let SearchOutcome::Found(found) = outcome else {
    ///     panic!("expected a path, got {outcome:?}");
    /// };
    /// assert_eq!(found.path.last(), Some(&NodeId(5)));
    /// assert_eq!(found.cost, Cost::from(5));
    ///
    /// // Three expansions cannot get that far, which says nothing about reachability.
    /// let limits = SearchLimits { max_expansions: 3, cancel: None };
    /// let outcome = search.find_path_with(&grid, NetId(0), source, targets, limits);
    /// assert_eq!(outcome, SearchOutcome::LimitReached);
    /// # Ok::<(), guide::space::grid::GridError>(())
    /// ```
    pub fn find_path_with<T>(
        &mut self,
        space: &T,
        net: NetId,
        sources: impl IntoIterator<Item = Source<S::Node>>,
        targets: impl IntoIterator<Item = Target<S::Node>>,
        limits: SearchLimits<'_>,
    ) -> SearchOutcome<'_, S::Node>
    where
        T: RoutingSpace<Node = S::Node>,
    {
        // The previous search's targets, and their positions in `target_positions`, are still
        // there to compare with until each slot is overwritten.
        mem::swap(&mut self.targets, &mut self.previous_targets);
        self.begin();
        let previous = self.generation - 1;
        let mut reuse = self.reusing && previous > 0;
        self.was_at.clear();
        self.kept.clear();
        if reuse {
            self.kept.resize(self.previous_targets.len(), false);
        }

        // Asked of `space` and not of a slot mapping, since a mapping might have no answer
        // but a slot number that is far too large to allocate.
        for target in targets.into_iter().filter(|t| space.contains(t.node)) {
            let slot = self.slots.slot(target.node);
            self.ensure_slot(slot, target.node);
            // Skipping duplicates here spares a lower bound per duplicate per node.
            if self.states[slot].target == self.generation {
                let kept = &mut self.targets[self.target_positions[slot]].cost;
                *kept = (*kept).min(target.cost);
            } else {
                if reuse {
                    // A target that was not one before ends it. A slot mapping may have given
                    // the slot to another node since, which the node itself tells apart.
                    let position = self.target_positions[slot];
                    reuse = self.states[slot].target == previous
                        && self
                            .previous_targets
                            .get(position)
                            .is_some_and(|before| before.node == target.node);
                    if reuse {
                        self.was_at.push(position);
                        self.kept[position] = true;
                    }
                }
                self.states[slot].target = self.generation;
                self.target_positions[slot] = self.targets.len();
                self.targets.push(target);
            }
        }
        // The lowest cost of each target has to be what it was, once duplicates are in.
        reuse = reuse
            && self
                .targets
                .iter()
                .zip(&self.was_at)
                .all(|(now, &position)| now.cost == self.previous_targets[position].cost);
        if reuse {
            let gone = self.previous_targets.iter().zip(&self.kept);
            self.removed
                .extend(gone.filter(|&(_, &kept)| !kept).map(|(&before, _)| before));
        }
        self.reusing = reuse;
        if self.targets.is_empty() {
            return SearchOutcome::Unreachable;
        }

        for source in sources.into_iter().filter(|s| space.contains(s.node)) {
            self.offer(space, net, source.node, source.cost, NO_PARENT);
        }

        loop {
            if !self.deferred.is_empty()
                && self
                    .heap
                    .peek()
                    .is_none_or(|entry| entry.f == Cost::MAX.key())
            {
                self.flush_deferred();
            }
            let Some(entry) = self.heap.pop() else {
                break;
            };
            if entry.slot & FINISH_BIT != 0 {
                let slot = entry.slot & !FINISH_BIT;
                // Read afresh, in case the node got cheaper by so little that its finish
                // came out the same as this one. Otherwise, a node that gets cheaper queues
                // a new finish, which comes first.
                let cost = self.states[slot].g + self.target_cost(slot);
                return SearchOutcome::Found(self.finish(slot, cost));
            }

            let state = self.states[entry.slot];
            if entry.inverted_g != !state.g.key() {
                self.stats.stale_pops += 1;
                continue;
            }

            // With an admissible bound, every entry that could still lead to a cheaper
            // target has an `f` no greater than that target's true cost, and so would
            // have come first. The module documentation covers what rounding does to this.
            if state.target == self.generation {
                let finishing = self.target_cost(entry.slot);
                if finishing == Cost::ZERO {
                    return SearchOutcome::Found(self.finish(entry.slot, state.g));
                }
                let total = state.g + finishing;
                self.push_entry(total, total, entry.slot | FINISH_BIT);
            }

            if self.stats.expanded >= limits.max_expansions {
                return SearchOutcome::LimitReached;
            }
            if self.stats.expanded.is_multiple_of(CANCEL_CHECK_INTERVAL)
                && limits
                    .cancel
                    .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                return SearchOutcome::Cancelled;
            }

            self.stats.expanded += 1;
            let node = self.nodes[entry.slot];
            // Taken out of `self` so that `offer` can borrow the rest; moving a `Vec`
            // does not touch its allocation.
            let mut steps = mem::take(&mut self.steps);
            steps.clear();
            space.expand(net, node, &mut steps);
            for step in &steps {
                self.offer(space, net, step.to, state.g + step.cost, entry.slot);
            }
            self.steps = steps;
        }
        SearchOutcome::Unreachable
    }

    /// Discards the previous search's state.
    fn begin(&mut self) {
        self.slots.clear();
        self.heap.clear();
        self.targets.clear();
        self.path.clear();
        self.next_sequence = 0;
        self.stats = SearchStats::default();
        self.deferred.clear();
        self.flushed = false;
        self.removed.clear();
        self.reusing = mem::take(&mut self.reuse_next);

        if self.generation == u32::MAX {
            // After 4 billion searches an old stamp could match again, so they all go.
            self.states.fill(SlotState::UNSEEN);
            self.generation = 0;
            self.reusing = false;
        }
        self.generation += 1;
    }

    /// Grows the arrays to include `slot`, which must be the slot of a node in the space.
    #[inline]
    fn ensure_slot(&mut self, slot: usize, node: S::Node) {
        if slot >= self.states.len() {
            self.states.resize(slot + 1, SlotState::UNSEEN);
            // Nodes have no default value, and `node` will do as a filler: entries are
            // only read for slots reached in the current generation, which overwrites them.
            self.nodes.resize(slot + 1, node);
            self.target_positions.resize(slot + 1, 0);
        }
    }

    /// Records a way to reach `node` at cost `g`, and queues it if that is an improvement.
    #[inline]
    fn offer<T>(&mut self, space: &T, net: NetId, node: S::Node, g: Cost, parent: usize)
    where
        T: RoutingSpace<Node = S::Node>,
    {
        let slot = self.slots.slot(node);
        self.ensure_slot(slot, node);

        let before = self.states[slot];
        if before.reached == self.generation {
            // A tie must not win: this keeps the first-found path, and it also keeps
            // parents acyclic. A child's `g` is never below its parent's, because adding
            // a cost never lowers a total, so closing a cycle would need a strict decrease
            // that cannot exist.
            if before.parent != DEFERRED && g >= before.g {
                return;
            }
        } else {
            let previous = self.generation - 1;
            let h = if before.target == self.generation {
                // Already the true remaining cost, and needs no call into the space.
                Cost::ZERO
            } else if self.reusing
                && before.reached == previous
                && before.target != previous
                && self.nodes[slot] == node
                && before.h < self.weighted_bound(space, net, node, &self.removed)
            {
                // The lowest bound came from a target that is still there, with the same
                // cost, as none of those gone were as low. A weighted bound never rounds
                // below another for a higher plain bound, so this holds for weights too.
                before.h
            } else {
                self.weighted_bound(space, net, node, &self.targets)
            };
            let state = &mut self.states[slot];
            state.reached = self.generation;
            state.h = h;
            self.nodes[slot] = node;
            // Set either way, as an old search may have left the mark.
            state.parent = if state.h == Cost::MAX && !self.flushed {
                DEFERRED
            } else {
                NO_PARENT
            };
        }
        let state = &mut self.states[slot];
        if state.parent == DEFERRED {
            // Only before the flush, which offers every node held back.
            self.deferred.push((slot, g, parent, self.next_sequence));
            self.next_sequence += 1;
            return;
        }
        state.g = g;
        state.parent = parent;

        let f = g + state.h;
        self.push_entry(f, g, slot);
    }

    /// Returns the weighted heuristic of `node` over `targets`.
    #[inline]
    fn weighted_bound<T>(
        &self,
        space: &T,
        net: NetId,
        node: S::Node,
        targets: &[Target<S::Node>],
    ) -> Cost
    where
        T: RoutingSpace<Node = S::Node>,
    {
        let bound = min_lower_bound(space, net, node, targets);
        if self.bound_weight == 1.0 {
            bound
        } else {
            Cost::new(bound.get() * self.bound_weight).unwrap_or(Cost::MAX)
        }
    }

    /// Makes the offers held back, in the order they were made, with the sequence numbers
    /// they were given then.
    fn flush_deferred(&mut self) {
        self.flushed = true;
        let deferred = mem::take(&mut self.deferred);
        for &(slot, g, parent, sequence) in &deferred {
            let state = &mut self.states[slot];
            if state.parent != DEFERRED && g >= state.g {
                continue;
            }
            state.g = g;
            state.parent = parent;
            let f = g + state.h;
            self.heap.push(HeapEntry {
                f: f.key(),
                inverted_g: !g.key(),
                sequence,
                slot,
            });
            self.stats.pushes += 1;
        }
        self.deferred = deferred;
        self.deferred.clear();
    }

    /// Queues an entry for `slot`, which may have the [`FINISH_BIT`] set.
    #[inline]
    fn push_entry(&mut self, f: Cost, g: Cost, slot: usize) {
        self.heap.push(HeapEntry {
            f: f.key(),
            inverted_g: !g.key(),
            sequence: self.next_sequence,
            slot,
        });
        self.next_sequence += 1;
        self.stats.pushes += 1;
    }

    /// Returns the finishing cost of the target in `slot`, which must be current.
    #[inline]
    fn target_cost(&self, slot: usize) -> Cost {
        self.targets[self.target_positions[slot]].cost
    }

    /// Builds the result for the target in `slot`.
    fn finish(&mut self, slot: usize, cost: Cost) -> Found<'_, S::Node> {
        let mut current = slot;
        while current != NO_PARENT {
            self.path.push(self.nodes[current]);
            current = self.states[current].parent;
        }
        self.path.reverse();
        Found {
            path: &self.path,
            cost,
        }
    }
}

/// Returns the search heuristic for `node`: the lowest bound over `targets`, each with the
/// cost of finishing at that target.
#[inline]
fn min_lower_bound<T: RoutingSpace>(
    space: &T,
    net: NetId,
    node: T::Node,
    targets: &[Target<T::Node>],
) -> Cost {
    let mut lowest = Cost::MAX;
    for target in targets {
        lowest = lowest.min(space.lower_bound(net, node, target.node) + target.cost);
        if lowest == Cost::ZERO {
            break;
        }
    }
    lowest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;
    use crate::space::{DenseSlots, InternedSlots};
    use core::cell::Cell;
    use proptest::prelude::*;

    const NET: NetId = NetId(0);

    /// An explicit graph with a tabulated lower bound, which counts its expansions.
    struct TestGraph {
        steps: Vec<Vec<Step<NodeId>>>,
        /// One row per `from`, one column per `to`. Empty means a zero bound.
        bounds: Vec<Vec<Cost>>,
        expansions: Cell<u64>,
    }

    impl TestGraph {
        fn new(nodes: usize, edges: &[(u32, u32, Cost)]) -> Self {
            let mut steps = vec![Vec::new(); nodes];
            for &(from, to, cost) in edges {
                steps[from as usize].push(Step {
                    to: NodeId(to),
                    cost,
                });
            }
            TestGraph {
                steps,
                bounds: Vec::new(),
                expansions: Cell::new(0),
            }
        }

        fn with_integer_edges(nodes: usize, edges: &[(u32, u32, u32)]) -> Self {
            let edges: Vec<_> = edges
                .iter()
                .map(|&(from, to, cost)| (from, to, Cost::from(cost)))
                .collect();
            TestGraph::new(nodes, &edges)
        }

        fn with_bound(mut self, bound: impl Fn(usize, usize) -> Cost) -> Self {
            let nodes = self.steps.len();
            self.bounds = (0..nodes)
                .map(|from| (0..nodes).map(|to| bound(from, to)).collect())
                .collect();
            self
        }

        /// Sets the bound towards target `to`, from a list of `(from, bound)` pairs.
        fn with_bounds_to(self, to: usize, list: &[(usize, u32)]) -> Self {
            self.with_bound(|from, target| {
                let found = list.iter().find(|&&(node, _)| node == from);
                if target == to {
                    found.map_or(Cost::ZERO, |&(_, bound)| Cost::from(bound))
                } else {
                    Cost::ZERO
                }
            })
        }

        /// The cheapest direct step between two nodes.
        fn step_cost(&self, from: NodeId, to: NodeId) -> Option<Cost> {
            self.steps[from.index()]
                .iter()
                .filter(|step| step.to == to)
                .map(|step| step.cost)
                .min()
        }
    }

    impl RoutingSpace for TestGraph {
        type Node = NodeId;
        type Resource = NodeId;
        type Slots = DenseSlots<NodeId>;
        type ResourceSlots = DenseSlots<NodeId>;

        fn new_slots(&self) -> Self::Slots {
            DenseSlots::new()
        }

        fn new_resource_slots(&self) -> Self::ResourceSlots {
            DenseSlots::new()
        }

        fn contains(&self, node: NodeId) -> bool {
            node.index() < self.steps.len()
        }

        fn expand(&self, _net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
            self.expansions.set(self.expansions.get() + 1);
            out.extend_from_slice(&self.steps[from.index()]);
        }

        fn lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> Cost {
            if self.bounds.is_empty() {
                Cost::ZERO
            } else {
                self.bounds[from.index()][to.index()]
            }
        }

        fn resources(&self, _: NetId, _: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
            out.push(to);
        }

        fn capacity(&self, _resource: NodeId) -> u32 {
            1
        }
    }

    /// A grid generated on demand, whose non-dense nodes need interned slots. Net 1 finds
    /// a wall at `x == 2` with a gap only at the far edge, so that nets see different
    /// spaces.
    struct LazyGrid {
        size: i32,
    }

    impl RoutingSpace for LazyGrid {
        type Node = (i32, i32);
        type Resource = (i32, i32);
        type Slots = InternedSlots<(i32, i32)>;
        type ResourceSlots = InternedSlots<(i32, i32)>;

        fn new_slots(&self) -> Self::Slots {
            InternedSlots::new()
        }

        fn new_resource_slots(&self) -> Self::ResourceSlots {
            InternedSlots::new()
        }

        fn contains(&self, node: (i32, i32)) -> bool {
            (0..self.size).contains(&node.0) && (0..self.size).contains(&node.1)
        }

        fn expand(&self, net: NetId, (x, y): (i32, i32), out: &mut Vec<Step<(i32, i32)>>) {
            for to in [(x + 1, y), (x, y + 1), (x - 1, y), (x, y - 1)] {
                let inside = (0..self.size).contains(&to.0) && (0..self.size).contains(&to.1);
                let walled = net == NetId(1) && to.0 == 2 && to.1 != self.size - 1;
                if inside && !walled {
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

    fn sources(nodes: &[u32]) -> Vec<Source<NodeId>> {
        nodes.iter().map(|&n| Source::new(NodeId(n))).collect()
    }

    fn targets(nodes: &[u32]) -> Vec<NodeId> {
        nodes.iter().map(|&n| NodeId(n)).collect()
    }

    /// Runs one search with a new `Search`, returning the path as plain numbers.
    fn route(space: &TestGraph, from: &[u32], to: &[u32]) -> Option<(Vec<u32>, f64)> {
        Search::new(space)
            .find_path(space, NET, sources(from), targets(to))
            .map(|found| {
                let path = found.path.iter().map(|node| node.0).collect();
                (path, found.cost.get())
            })
    }

    #[test]
    fn finds_the_hand_checked_shortest_path() {
        //  0 --1-- 1 --1-- 2
        //  |       |       |
        //  4       1       5
        //  |       |       |
        //  3 --1-- 4 --7-- 5
        let space = TestGraph::with_integer_edges(
            6,
            &[
                (0, 1, 1),
                (1, 2, 1),
                (0, 3, 4),
                (1, 4, 1),
                (2, 5, 5),
                (3, 4, 1),
                (4, 5, 7),
            ],
        );
        assert_eq!(route(&space, &[0], &[5]), Some((vec![0, 1, 2, 5], 7.0)));
        assert_eq!(route(&space, &[0], &[4]), Some((vec![0, 1, 4], 2.0)));
        assert_eq!(route(&space, &[3], &[5]), Some((vec![3, 4, 5], 8.0)));
    }

    #[test]
    fn a_star_agrees_with_dijkstra_and_expands_less() {
        // A row of 20 nodes, searched from the middle: Dijkstra spreads both ways.
        let edges: Vec<_> = (0..19u32)
            .flat_map(|n| [(n, n + 1, 1), (n + 1, n, 1)])
            .collect();
        let dijkstra = TestGraph::with_integer_edges(20, &edges);
        let a_star = TestGraph::with_integer_edges(20, &edges)
            .with_bound(|from, to| Cost::from(from.abs_diff(to) as u32));

        let mut plain = Search::new(&dijkstra);
        let mut guided = Search::new(&a_star);
        let slow = plain
            .find_path(&dijkstra, NET, sources(&[10]), targets(&[15]))
            .map(|found| (found.path.to_vec(), found.cost));
        let fast = guided
            .find_path(&a_star, NET, sources(&[10]), targets(&[15]))
            .map(|found| (found.path.to_vec(), found.cost));
        assert_eq!(slow, fast);
        assert_eq!(guided.stats().expanded, 5);
        assert!(plain.stats().expanded > guided.stats().expanded);
    }

    #[test]
    fn multiple_targets_reach_the_cheapest() {
        let space = TestGraph::with_integer_edges(4, &[(0, 1, 5), (0, 2, 3), (2, 3, 1)]);
        assert_eq!(route(&space, &[0], &[1, 3]), Some((vec![0, 2, 3], 4.0)));
        assert_eq!(route(&space, &[0], &[3, 1]), Some((vec![0, 2, 3], 4.0)));
    }

    #[test]
    fn the_heuristic_is_the_minimum_over_targets() {
        // The bound to target 2 alone would be inadmissible for a search that may also
        // finish at target 3, so taking any bound but the lowest would miss the best path.
        let space = TestGraph::with_integer_edges(4, &[(0, 1, 1), (1, 3, 1), (0, 2, 3)])
            .with_bound(|from, to| match (from, to) {
                (1, 2) => Cost::from(100),
                _ => Cost::ZERO,
            });
        assert_eq!(route(&space, &[0], &[2, 3]), Some((vec![0, 1, 3], 2.0)));
    }

    #[test]
    fn starting_costs_count_towards_the_total() {
        let space = TestGraph::with_integer_edges(3, &[(0, 2, 1), (1, 2, 5)]);
        let mut search = Search::new(&space);

        // Source 0 is nearer, but its starting cost makes it the worse choice.
        let found = search
            .find_path(
                &space,
                NET,
                [
                    Source::with_cost(NodeId(0), Cost::from(10)),
                    Source::with_cost(NodeId(1), Cost::from(2)),
                ],
                targets(&[2]),
            )
            .unwrap();
        assert_eq!(found.path, targets(&[1, 2]));
        assert_eq!(found.cost, Cost::from(7));
    }

    #[test]
    fn duplicate_sources_keep_the_lowest_cost_in_any_order() {
        let space = TestGraph::with_integer_edges(2, &[(0, 1, 1)]);
        let mut search = Search::new(&space);
        let cheap = Source::with_cost(NodeId(0), Cost::from(2));
        let dear = Source::with_cost(NodeId(0), Cost::from(9));

        for list in [[cheap, dear], [dear, cheap]] {
            let found = search.find_path(&space, NET, list, targets(&[1])).unwrap();
            assert_eq!(found.path, targets(&[0, 1]));
            assert_eq!(found.cost, Cost::from(3));
        }
    }

    #[test]
    fn duplicate_targets_change_nothing() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 2), (1, 2, 2)]);
        assert_eq!(route(&space, &[0], &[2, 2, 2]), route(&space, &[0], &[2]));

        let mut search = Search::new(&space);
        search.find_path(&space, NET, sources(&[0]), targets(&[2, 2]));
        assert_eq!(search.targets.len(), 1);
    }

    #[test]
    fn a_source_that_is_a_target_is_a_path_of_one_node() {
        let space = TestGraph::with_integer_edges(2, &[(0, 1, 1)]);
        let mut search = Search::new(&space);
        let found = search
            .find_path(&space, NET, sources(&[0]), targets(&[0]))
            .unwrap();
        assert_eq!(found.path, targets(&[0]));
        assert_eq!(found.cost, Cost::ZERO);
        assert_eq!(search.stats().expanded, 0);
    }

    #[test]
    fn a_source_that_is_a_target_can_be_beaten_by_another_source() {
        let space = TestGraph::with_integer_edges(2, &[(1, 0, 1)]);
        let mut search = Search::new(&space);

        let list = [
            Source::with_cost(NodeId(0), Cost::from(10)),
            Source::new(NodeId(1)),
        ];
        let found = search.find_path(&space, NET, list, targets(&[0])).unwrap();
        assert_eq!(found.path, targets(&[1, 0]));
        assert_eq!(found.cost, Cost::ONE);
    }

    #[test]
    fn nothing_to_do_or_no_way_there_finds_nothing() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1)]);
        assert_eq!(route(&space, &[], &[1]), None);
        assert_eq!(route(&space, &[0], &[]), None);
        assert_eq!(route(&space, &[], &[]), None);
        assert_eq!(route(&space, &[0], &[2]), None);
        // Edges are directed.
        assert_eq!(route(&space, &[1], &[0]), None);
    }

    #[test]
    fn costs_near_the_maximum_saturate_without_losing_paths() {
        let huge = Cost::MAX;
        let space = TestGraph::new(3, &[(0, 1, huge), (1, 2, huge)]);
        assert_eq!(route(&space, &[0], &[2]).unwrap().0, [0, 1, 2]);
        assert_eq!(route(&space, &[0], &[2]).unwrap().1, f64::MAX);

        // A node first reached at exactly the maximum is reached, not unset.
        let mut search = Search::new(&space);
        let list = [Source::with_cost(NodeId(2), huge)];
        let found = search.find_path(&space, NET, list, targets(&[2])).unwrap();
        assert_eq!(found.cost, huge);

        // A saturated path must lose to a representable one, however long.
        let cheap = Cost::ONE;
        let space = TestGraph::new(
            4,
            &[(0, 3, huge), (0, 1, cheap), (1, 2, cheap), (2, 3, cheap)],
        );
        assert_eq!(route(&space, &[0], &[3]), Some((vec![0, 1, 2, 3], 3.0)));

        // A saturating bound must not stop a source from being explored.
        let space = TestGraph::new(2, &[(0, 1, huge)]).with_bound(|_, _| huge);
        assert_eq!(route(&space, &[0], &[1]).unwrap().1, f64::MAX);
    }

    #[test]
    fn a_node_whose_bound_is_the_maximum_is_still_searched_when_nothing_else_is_left() {
        // The only way to 3 is through 1, whose bound says there is none. 2 is a dead end.
        let space = TestGraph::with_integer_edges(4, &[(0, 1, 1), (0, 2, 1), (1, 3, 1)])
            .with_bound(|from, _| if from == 1 { Cost::MAX } else { Cost::ZERO });
        let mut search = Search::new(&space);
        let found = search
            .find_path(&space, NET, sources(&[0]), targets(&[3]))
            .unwrap();
        assert_eq!(found.path, targets(&[0, 1, 3]));
        assert_eq!(found.cost, Cost::from(2));
        assert_eq!(search.stats().expanded, 3);

        // Where the target comes first, a node held back is never queued at all.
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (0, 2, 1)])
            .with_bound(|from, _| if from == 1 { Cost::MAX } else { Cost::ZERO });
        let found = search
            .find_path(&space, NET, sources(&[0]), targets(&[2]))
            .unwrap();
        assert_eq!(found.path, targets(&[0, 2]));
        assert_eq!(search.stats().pushes, 2);
    }

    #[test]
    fn a_weighted_bound_goes_straight_for_the_target() {
        // 4 is a dead end that looks close, and 1 is the way, a little further along.
        let space = TestGraph::with_integer_edges(5, &[(0, 4, 0), (0, 1, 2), (1, 3, 2)])
            .with_bounds_to(3, &[(1, 2), (4, 3)]);
        let mut search = Search::new(&space);
        let route = |search: &mut Search<TestGraph>| {
            let found = search.find_path(&space, NET, sources(&[0]), targets(&[3]));
            (found.map(|found| found.cost), search.stats().expanded)
        };
        assert_eq!(route(&mut search), (Some(Cost::from(4)), 3));
        // At 3, 1 comes first, and 4 is never expanded.
        search.set_bound_weight(3.0);
        assert_eq!(route(&mut search), (Some(Cost::from(4)), 2));
        // Anything else is the plain bound, and 0 is Dijkstra's algorithm.
        for weight in [f64::NAN, -1.0, f64::INFINITY] {
            search.set_bound_weight(weight);
            assert_eq!(search.bound_weight, 1.0);
        }
        search.set_bound_weight(0.0);
        assert_eq!(route(&mut search), (Some(Cost::from(4)), 3));
    }

    #[test]
    fn zero_cost_cycles_and_self_loops_terminate() {
        let space = TestGraph::with_integer_edges(
            4,
            &[(0, 0, 0), (0, 1, 0), (1, 2, 0), (2, 0, 0), (2, 3, 1)],
        );
        assert_eq!(route(&space, &[0], &[3]), Some((vec![0, 1, 2, 3], 1.0)));
    }

    #[test]
    fn re_opens_a_node_when_an_inconsistent_bound_found_it_too_expensively() {
        // The bound is admissible but inconsistent: from 1 it says 4, and from 2, one step
        // of cost 1 further along, it says 0. So 2 is expanded at g = 3 by way of the
        // direct edge, before the route through 1 finds it at g = 2. Without re-opening 2
        // the answer would be 6.
        let space = TestGraph::with_integer_edges(4, &[(0, 2, 3), (0, 1, 1), (1, 2, 1), (2, 3, 3)])
            .with_bounds_to(3, &[(1, 4)]);
        let mut search = Search::new(&space);
        let found = search
            .find_path(&space, NET, sources(&[0]), targets(&[3]))
            .unwrap();
        assert_eq!(found.path, targets(&[0, 1, 2, 3]));
        assert_eq!(found.cost, Cost::from(5));

        // 0, 2, 1, and 2 again.
        assert_eq!(search.stats().expanded, 4);
        assert_eq!(space.expansions.get(), 4);
        // The target's first entry, at g = 6, is what gets superseded.
        assert_eq!(search.stats().stale_pops, 0);
        assert_eq!(search.stats().pushes, 6);
    }

    #[test]
    fn skips_stale_entries() {
        // 2 is first queued at g = 9, then replaced at g = 2 before its entry comes up.
        let space = TestGraph::with_integer_edges(4, &[(0, 2, 9), (0, 1, 1), (1, 2, 1), (2, 3, 9)]);
        let mut search = Search::new(&space);
        let found = search
            .find_path(&space, NET, sources(&[0]), targets(&[3]))
            .unwrap();
        assert_eq!(found.cost, Cost::from(11));
        assert_eq!(search.stats().expanded, 3);
        // The g = 9 entry leaves the queue before the target at g = 11.
        assert_eq!(search.stats().stale_pops, 1);
    }

    #[test]
    fn equal_paths_are_resolved_by_expansion_order() {
        let a_first =
            TestGraph::with_integer_edges(4, &[(0, 1, 1), (0, 2, 1), (1, 3, 1), (2, 3, 1)]);
        assert_eq!(route(&a_first, &[0], &[3]).unwrap().0, [0, 1, 3]);

        let b_first =
            TestGraph::with_integer_edges(4, &[(0, 2, 1), (0, 1, 1), (1, 3, 1), (2, 3, 1)]);
        assert_eq!(route(&b_first, &[0], &[3]).unwrap().0, [0, 2, 3]);
    }

    #[test]
    fn equal_f_prefers_the_larger_g() {
        // 1 and the target both have f = 2, and the target has the larger g. So the search
        // ends without expanding 1, even though 1 was inserted first.
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (0, 2, 2), (1, 2, 1)])
            .with_bounds_to(2, &[(1, 1)]);
        let mut search = Search::new(&space);
        let found = search
            .find_path(&space, NET, sources(&[0]), targets(&[2]))
            .unwrap();
        assert_eq!(found.path, targets(&[0, 2]));
        assert_eq!(search.stats().expanded, 1);
    }

    #[test]
    fn a_target_needs_no_lower_bound() {
        // An inadmissible bound on a target must not get in the way of reaching it.
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (1, 2, 1), (0, 2, 5)])
            .with_bound(|from, to| {
                if from == to {
                    Cost::from(100)
                } else {
                    Cost::ZERO
                }
            });
        assert_eq!(route(&space, &[0], &[2]), Some((vec![0, 1, 2], 2.0)));
    }

    #[test]
    fn an_inadmissible_bound_gives_a_valid_path_of_the_reported_cost() {
        // The bound from 1 overestimates, so the search settles for the direct edge.
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (1, 2, 1), (0, 2, 5)])
            .with_bounds_to(2, &[(1, 100)]);
        assert_eq!(route(&space, &[0], &[2]), Some((vec![0, 2], 5.0)));
    }

    #[test]
    fn rounding_stays_within_the_documented_bound() {
        // Two steps of half an ulp each vanish when added to 1 one at a time, but not when
        // summed first. So 1 --> 2 --> 3 has a left-to-right cost of exactly 1, while the
        // exact bound from 1 makes it look like 1 + 2^-52, which is no better than the
        // direct edge.
        let half_ulp = Cost::new(2f64.powi(-53)).unwrap();
        let direct = Cost::new(1.0 + 2f64.powi(-52)).unwrap();
        let edges = [
            (0, 1, Cost::ONE),
            (1, 2, half_ulp),
            (2, 3, half_ulp),
            (0, 3, direct),
        ];

        // Dijkstra finds the true minimum of the left-to-right cost.
        let space = TestGraph::new(4, &edges);
        assert_eq!(route(&space, &[0], &[3]), Some((vec![0, 1, 2, 3], 1.0)));

        // A* takes the direct edge, whose cost equals the exact sum of either path.
        let space = TestGraph::new(4, &edges).with_bound(|from, _| match from {
            1 => half_ulp + half_ulp,
            2 => half_ulp,
            _ => Cost::ZERO,
        });
        assert_eq!(route(&space, &[0], &[3]).unwrap().0, [0, 3]);
        assert_eq!(route(&space, &[0], &[3]).unwrap().1, direct.get());
    }

    #[test]
    fn reuse_leaves_no_stale_state() {
        let space = TestGraph::with_integer_edges(
            5,
            &[(0, 1, 1), (1, 2, 1), (3, 1, 7), (1, 4, 1), (2, 4, 1)],
        );
        let mut search = Search::new(&space);
        let cost = |found: Option<Found<'_, NodeId>>| found.map(|found| found.cost.get());

        // Old costs that are lower than the new ones must not block anything.
        let first = search.find_path(&space, NET, sources(&[0]), targets(&[2]));
        assert_eq!(cost(first), Some(2.0));
        let found = search
            .find_path(&space, NET, sources(&[3]), targets(&[2]))
            .unwrap();
        assert_eq!(found.path, targets(&[3, 1, 2]));
        assert_eq!(found.cost, Cost::from(8));

        // An old target is no longer a target, and old sources are no longer sources.
        let found = search
            .find_path(&space, NET, sources(&[1]), targets(&[4]))
            .unwrap();
        assert_eq!(found.path, targets(&[1, 4]));
        assert_eq!(
            search.find_path(&space, NET, sources(&[4]), targets(&[2])),
            None
        );

        // Failing and empty searches leave nothing behind either.
        assert_eq!(
            search.find_path(&space, NET, sources(&[]), targets(&[2])),
            None
        );
        assert_eq!(search.stats(), SearchStats::default());
        let found = search.find_path(&space, NET, sources(&[0]), targets(&[1]));
        assert_eq!(cost(found), Some(1.0));
    }

    #[test]
    fn interned_slots_are_renumbered_between_searches() {
        let space = LazyGrid { size: 6 };
        let mut reused = Search::new(&space);

        let cases = [
            (NET, (0, 0), (5, 5), 10.0),
            // A different first-seen order gives every node a new slot.
            (NET, (5, 5), (0, 3), 7.0),
            // Through the wall's gap at y = 5.
            (NetId(1), (0, 0), (4, 0), 14.0),
            // Back again, with a smaller search than before.
            (NET, (0, 0), (0, 1), 1.0),
        ];
        for (net, from, to, cost) in cases {
            let source = [Source::new(from)];
            let found = reused.find_path(&space, net, source, [to]).unwrap();
            assert_eq!(found.cost.get(), cost);
            assert_eq!(found.path.first(), Some(&from));
            assert_eq!(found.path.last(), Some(&to));
            assert_eq!(found.path.len(), cost as usize + 1);
            let path = found.path.to_vec();
            let stats = reused.stats();

            let mut fresh = Search::new(&space);
            let again = fresh.find_path(&space, net, source, [to]).unwrap();
            assert_eq!(again.path, path);
            assert_eq!(fresh.stats(), stats);
        }
    }

    #[test]
    fn generation_wraparound_forgets_old_stamps() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (1, 2, 1)]);
        let mut search = Search::new(&space);
        let expected = (targets(&[0, 1, 2]), Cost::from(2));

        search.find_path(&space, NET, sources(&[0]), targets(&[1]));
        // Make the stamps left behind by that search match the generation after the wrap.
        search.generation = u32::MAX;
        for state in &mut search.states {
            if state.reached != 0 {
                state.reached = 1;
            }
            if state.target != 0 {
                state.target = 1;
            }
        }

        for generation in [1, 2] {
            let found = search
                .find_path(&space, NET, sources(&[0]), targets(&[2]))
                .unwrap();
            assert_eq!((found.path.to_vec(), found.cost), expected);
            assert_eq!(search.generation, generation);
        }
    }

    /// Everything a search could have allocated, to spot growth.
    fn capacities<S: RoutingSpace>(search: &Search<S>) -> [usize; 6] {
        [
            search.states.capacity(),
            search.nodes.capacity(),
            search.heap.entries.capacity(),
            search.targets.capacity(),
            search.steps.capacity(),
            search.path.capacity(),
        ]
    }

    #[test]
    fn nodes_outside_the_space_are_ignored_without_allocating() {
        let far = u32::MAX;
        let grid = crate::space::grid::Grid::new(3, 1, 1).unwrap();
        let graph = crate::space::graph::GraphBuilder::new(3).build();
        let mut on_grid = Search::new(&grid);
        let mut on_graph = Search::new(&graph);

        // Neither would survive trying to allocate state for a slot near `u32::MAX`.
        for (from, to) in [(&[far][..], &[0][..]), (&[0], &[far]), (&[far], &[far])] {
            let found = on_grid.find_path(&grid, NET, sources(from), targets(to));
            assert_eq!(found, None);
            assert_eq!(on_grid.stats(), SearchStats::default());
            let found = on_graph.find_path(&graph, NET, sources(from), targets(to));
            assert_eq!(found, None);
        }
        // Node 0 is all that any of those could have allocated for.
        assert_eq!(on_grid.states.len(), 1);
        assert_eq!(on_graph.states.len(), 1);

        // Through `Target`, with a cost that would otherwise be queued.
        let outcome = on_grid.find_path_with(
            &grid,
            NET,
            sources(&[0]),
            with_costs(&[(far, 3), (far - 1, 0)]),
            SearchLimits::NONE,
        );
        assert_eq!(outcome, SearchOutcome::Unreachable);
        assert_eq!(on_grid.stats().pushes, 0);

        // A limit of no expansions does not turn this into running out of limits.
        let limits = SearchLimits {
            max_expansions: 0,
            cancel: None,
        };
        let outcome =
            on_grid.find_path_with(&grid, NET, sources(&[far]), with_costs(&[(1, 0)]), limits);
        assert_eq!(outcome, SearchOutcome::Unreachable);
    }

    #[test]
    fn nodes_outside_the_space_do_not_get_in_the_way_of_those_inside() {
        let far = u32::MAX;
        let grid = crate::space::grid::Grid::new(4, 1, 1).unwrap();
        let mut search = Search::new(&grid);
        let expected = (targets(&[1, 2, 3]), Cost::from(2));

        let found = search
            .find_path(&grid, NET, sources(&[far, 1, far]), targets(&[far, 3]))
            .unwrap();
        assert_eq!((found.path.to_vec(), found.cost), expected);
        let pushes = search.stats().pushes;

        // The same work as without them, at the same cost and by the same path.
        let found = search
            .find_path(&grid, NET, sources(&[1]), targets(&[3]))
            .unwrap();
        assert_eq!((found.path.to_vec(), found.cost), expected);
        assert_eq!(search.stats().pushes, pushes);
        assert!(search.states.len() <= 4);
    }

    #[test]
    fn a_blocked_cell_is_in_the_space_and_a_foreign_node_is_not() {
        let far = u32::MAX;
        let mut grid = crate::space::grid::Grid::new(2, 1, 1).unwrap();
        grid.set_blocked(0, 0, 0, true).unwrap();
        let mut search = Search::new(&grid);
        // A blocked cell is still a path of one node, which the validator then rejects.
        let found = search.find_path(&grid, NET, sources(&[0]), targets(&[0]));
        assert_eq!(found.map(|found| found.path.len()), Some(1));
        assert_eq!(
            search.find_path(&grid, NET, sources(&[far]), targets(&[far])),
            None
        );
    }

    #[test]
    fn interned_slots_are_not_assigned_to_nodes_outside_the_space() {
        let space = LazyGrid { size: 4 };
        let mut search = Search::new(&space);
        let outside = (-1, 0);
        let found = search.find_path(&space, NET, [Source::new(outside)], [outside]);
        assert_eq!(found, None);
        assert_eq!(search.slots.get(outside), None);
        let found = search.find_path(&space, NET, [Source::new((0, 0))], [outside, (1, 0)]);
        assert_eq!(found.map(|found| found.cost), Some(Cost::ONE));
        assert_eq!(search.slots.get(outside), None);
    }

    #[test]
    fn repeated_searches_do_not_grow_the_buffers() {
        let edges: Vec<_> = (0..99u32)
            .flat_map(|n| [(n, n + 1, 1), (n + 1, n, 1)])
            .collect();
        let space = TestGraph::with_integer_edges(100, &edges);
        let mut search = Search::new(&space);

        search.find_path(&space, NET, sources(&[0]), targets(&[99]));
        let grown = capacities(&search);
        assert!(grown.iter().all(|&capacity| capacity > 0));
        for _ in 0..10 {
            search.find_path(&space, NET, sources(&[0]), targets(&[99]));
            search.find_path(&space, NET, sources(&[50]), targets(&[60]));
            assert_eq!(capacities(&search), grown);
        }
    }

    fn entry(f: u64, inverted_g: u64, sequence: u64) -> HeapEntry {
        HeapEntry {
            f,
            inverted_g,
            sequence,
            slot: 0,
        }
    }

    #[test]
    fn entries_compare_by_f_then_larger_g_then_insertion() {
        // Ordinary numbers for the keys, with g inverted as the search would.
        let (low_g, high_g) = (!1, !2);
        assert!(entry(1, high_g, 9).precedes(&entry(2, high_g, 0)));
        assert!(entry(1, high_g, 9).precedes(&entry(1, low_g, 0)));
        assert!(entry(1, low_g, 0).precedes(&entry(1, low_g, 1)));
        assert!(!entry(1, low_g, 1).precedes(&entry(1, low_g, 0)));
    }

    /// The heap's job done the slow and obvious way.
    fn pop_first(model: &mut Vec<HeapEntry>) -> Option<HeapEntry> {
        (0..model.len())
            .reduce(|a, b| if model[b].precedes(&model[a]) { b } else { a })
            .map(|index| model.swap_remove(index))
    }

    /// A random graph, with everything a search needs.
    #[derive(Clone, Debug)]
    struct Case {
        nodes: usize,
        edges: Vec<(u32, u32, u32)>,
        sources: Vec<(u32, u32)>,
        targets: Vec<u32>,
        /// Scales the bound for each `from`/`to` pair, as a fraction of 255. Being random
        /// for each node, this makes the bound inconsistent.
        fractions: Vec<u8>,
    }

    fn case() -> impl Strategy<Value = Case> {
        (1..=12usize).prop_flat_map(|nodes| {
            let node = 0..nodes as u32;
            (
                prop::collection::vec((node.clone(), node.clone(), 0..=20u32), 0..40),
                prop::collection::vec((node.clone(), 0..=20u32), 0..4),
                prop::collection::vec(node, 0..4),
                prop::collection::vec(any::<u8>(), nodes * nodes),
            )
                .prop_map(move |(edges, sources, targets, fractions)| Case {
                    nodes,
                    edges,
                    sources,
                    targets,
                    fractions,
                })
        })
    }

    /// Bellman-Ford over the same left-to-right cost as the search, from starting costs
    /// given per node. Repeats until nothing improves, so needs no ordering assumptions.
    fn bellman_ford(
        space: &TestGraph,
        starts: impl IntoIterator<Item = (u32, Cost)>,
    ) -> Vec<Option<Cost>> {
        let mut best: Vec<Option<Cost>> = vec![None; space.steps.len()];
        for (node, cost) in starts {
            let slot = &mut best[node as usize];
            *slot = Some(slot.map_or(cost, |old| old.min(cost)));
        }
        loop {
            let mut changed = false;
            for from in 0..space.steps.len() {
                let Some(g) = best[from] else { continue };
                for step in &space.steps[from] {
                    let new = g + step.cost;
                    let slot = &mut best[step.to.index()];
                    if slot.is_none_or(|old| new < old) {
                        *slot = Some(new);
                        changed = true;
                    }
                }
            }
            if !changed {
                return best;
            }
        }
    }

    /// The best cost to any target according to brute force.
    fn brute_force_cost(
        space: &TestGraph,
        starts: impl IntoIterator<Item = (u32, Cost)>,
        goals: &[u32],
    ) -> Option<Cost> {
        let best = bellman_ford(space, starts);
        goals.iter().filter_map(|&n| best[n as usize]).min()
    }

    /// Checks that `path` is a real path from a source to a target, and returns its cost.
    fn path_cost(
        space: &TestGraph,
        starts: &[(u32, Cost)],
        goals: &[u32],
        path: &[NodeId],
    ) -> Result<Cost, TestCaseError> {
        let (first, last) = (path.first().unwrap(), path.last().unwrap());
        prop_assert!(goals.contains(&last.0));
        let start = starts
            .iter()
            .filter(|&&(node, _)| node == first.0)
            .map(|&(_, cost)| cost)
            .min();
        prop_assert!(start.is_some(), "path does not begin at a source");

        let mut sum = start.unwrap();
        for pair in path.windows(2) {
            let cost = space.step_cost(pair[0], pair[1]);
            prop_assert!(cost.is_some(), "path uses a missing step");
            sum = sum + cost.unwrap();
        }
        Ok(sum)
    }

    /// The tolerance for comparing results that were rounded differently, which is far
    /// above the documented bound for these small graphs and far below their step costs.
    fn close(a: Cost, b: Cost) -> bool {
        (a.get() - b.get()).abs() <= 1e-12 * a.get().max(b.get())
    }

    /// Builds a search-friendly list of sources.
    fn cost_sources(starts: &[(u32, Cost)]) -> Vec<Source<NodeId>> {
        starts
            .iter()
            .map(|&(node, cost)| Source::with_cost(NodeId(node), cost))
            .collect()
    }

    fn integer_starts(case: &Case) -> Vec<(u32, Cost)> {
        case.sources
            .iter()
            .map(|&(node, cost)| (node, Cost::from(cost)))
            .collect()
    }

    /// The case's graph, with a bound of a random fraction of the true distance.
    ///
    /// Integer costs stay exact because the fraction is rounded down to an integer. Where
    /// a target cannot be reached any bound is admissible, so a large one is used.
    fn with_integer_bound(case: &Case) -> TestGraph {
        let plain = TestGraph::with_integer_edges(case.nodes, &case.edges);
        let distances: Vec<_> = (0..case.nodes as u32)
            .map(|from| bellman_ford(&plain, [(from, Cost::ZERO)]))
            .collect();
        plain.with_bound(|from, to| {
            let fraction = u32::from(case.fractions[from * case.nodes + to]);
            match distances[from][to] {
                Some(distance) => Cost::from((distance.get() as u32) * fraction / 255),
                None => Cost::from(1000 + fraction),
            }
        })
    }

    /// The same graph with step costs that round when added, scaled by `scale`, and a
    /// bound that is a fraction of the true distance as far as rounding allows.
    fn with_fractional_costs(case: &Case, scale: f64) -> TestGraph {
        let edges: Vec<_> = case
            .edges
            .iter()
            .map(|&(from, to, cost)| {
                let cost = Cost::new(f64::from(cost) * 0.1 * scale).unwrap();
                (from, to, cost)
            })
            .collect();
        let plain = TestGraph::new(case.nodes, &edges);
        let distances: Vec<_> = (0..case.nodes as u32)
            .map(|from| bellman_ford(&plain, [(from, Cost::ZERO)]))
            .collect();
        plain.with_bound(|from, to| {
            let fraction = f64::from(case.fractions[from * case.nodes + to]) / 255.0;
            match distances[from][to] {
                Some(distance) => Cost::new(distance.get() * fraction).unwrap(),
                None => Cost::MAX,
            }
        })
    }

    #[derive(Debug, PartialEq, Eq)]
    enum SearchEnd {
        Unreachable,
        LimitReached,
        Cancelled,
    }

    fn outcome_summary(outcome: SearchOutcome<'_, NodeId>) -> Result<(Vec<u32>, f64), SearchEnd> {
        match outcome {
            SearchOutcome::Found(found) => {
                Ok((found.path.iter().map(|n| n.0).collect(), found.cost.get()))
            }
            SearchOutcome::Unreachable => Err(SearchEnd::Unreachable),
            SearchOutcome::LimitReached => Err(SearchEnd::LimitReached),
            SearchOutcome::Cancelled => Err(SearchEnd::Cancelled),
        }
    }

    fn with_costs(list: &[(u32, u32)]) -> Vec<Target<NodeId>> {
        list.iter()
            .map(|&(node, cost)| Target::with_cost(NodeId(node), Cost::from(cost)))
            .collect()
    }

    /// Runs one unlimited search with target costs.
    fn route_to(space: &TestGraph, from: &[u32], to: &[(u32, u32)]) -> Option<(Vec<u32>, f64)> {
        let mut search = Search::new(space);
        let outcome = search.find_path_with(
            space,
            NET,
            sources(from),
            with_costs(to),
            SearchLimits::NONE,
        );
        outcome_summary(outcome).ok()
    }

    #[test]
    fn target_costs_count_towards_the_choice_and_the_total() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (0, 2, 5)]);
        assert_eq!(
            route_to(&space, &[0], &[(1, 9), (2, 0)]),
            Some((vec![0, 2], 5.0))
        );
        assert_eq!(
            route_to(&space, &[0], &[(1, 3), (2, 9)]),
            Some((vec![0, 1], 4.0))
        );
    }

    #[test]
    fn a_dear_target_can_be_passed_through_to_a_cheap_one() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (1, 2, 1)]);
        assert_eq!(
            route_to(&space, &[0], &[(1, 9), (2, 0)]),
            Some((vec![0, 1, 2], 2.0))
        );
    }

    #[test]
    fn a_source_that_is_a_costed_target_pays_its_cost_or_loses_to_a_cheaper_one() {
        let space = TestGraph::with_integer_edges(2, &[(0, 1, 1)]);
        assert_eq!(route_to(&space, &[0], &[(0, 2)]), Some((vec![0], 2.0)));
        assert_eq!(
            route_to(&space, &[0], &[(0, 2), (1, 0)]),
            Some((vec![0, 1], 1.0))
        );
    }

    #[test]
    fn duplicate_targets_keep_the_lowest_cost_in_any_order() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (0, 2, 4)]);
        for list in [[(1, 9), (1, 1), (2, 0)], [(1, 1), (1, 9), (2, 0)]] {
            assert_eq!(route_to(&space, &[0], &list), Some((vec![0, 1], 2.0)));
        }
    }

    #[test]
    fn target_costs_keep_the_heuristic_admissible() {
        // The bound from 1 to 2 is exact, and adding each target's own cost must not make
        // the search give up on the route through 1, which is the cheapest overall.
        let space = TestGraph::with_integer_edges(4, &[(0, 1, 1), (1, 2, 1), (0, 3, 2)])
            .with_bounds_to(2, &[(0, 2), (1, 1)]);
        assert_eq!(
            route_to(&space, &[0], &[(2, 1), (3, 5)]),
            Some((vec![0, 1, 2], 3.0))
        );
    }

    #[test]
    fn zero_target_costs_leave_expansions_as_they_were() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (1, 2, 1)]);
        let mut search = Search::new(&space);
        search.find_path(&space, NET, sources(&[0]), targets(&[1]));
        let plain = search.stats();
        let outcome = search.find_path_with(
            &space,
            NET,
            sources(&[0]),
            with_costs(&[(1, 0)]),
            SearchLimits::NONE,
        );
        assert_eq!(outcome_summary(outcome), Ok((vec![0, 1], 1.0)));
        assert_eq!(search.stats(), plain);
    }

    #[test]
    fn limits_tell_running_out_apart_from_having_no_path() {
        let space = TestGraph::with_integer_edges(5, &[(0, 1, 1), (1, 2, 1), (2, 3, 1)]);
        let mut search = Search::new(&space);
        let limit = |max_expansions| SearchLimits {
            max_expansions,
            cancel: None,
        };

        // Reaching 3 takes exactly three expansions.
        for (allowed, expected) in [
            (0, Err(SearchEnd::LimitReached)),
            (2, Err(SearchEnd::LimitReached)),
            (3, Ok((vec![0, 1, 2, 3], 3.0))),
            (4, Ok((vec![0, 1, 2, 3], 3.0))),
        ] {
            let outcome = search.find_path_with(
                &space,
                NET,
                sources(&[0]),
                with_costs(&[(3, 0)]),
                limit(allowed),
            );
            assert_eq!(outcome_summary(outcome), expected, "{allowed}");
        }

        // Exploring everything takes four, and proves that 4 cannot be reached.
        for (allowed, expected) in [(3, SearchEnd::LimitReached), (4, SearchEnd::Unreachable)] {
            let outcome = search.find_path_with(
                &space,
                NET,
                sources(&[0]),
                with_costs(&[(4, 0)]),
                limit(allowed),
            );
            assert_eq!(outcome_summary(outcome), Err(expected), "{allowed}");
        }

        // Needing no expansions is possible with none allowed.
        let outcome =
            search.find_path_with(&space, NET, sources(&[0]), with_costs(&[(0, 0)]), limit(0));
        assert_eq!(outcome_summary(outcome), Ok((vec![0], 0.0)));
        // As is seeing that there is nothing to search.
        let outcome =
            search.find_path_with(&space, NET, sources(&[]), with_costs(&[(0, 0)]), limit(0));
        assert_eq!(outcome_summary(outcome), Err(SearchEnd::Unreachable));
    }

    #[test]
    fn a_set_flag_stops_a_search_before_it_expands() {
        let space = TestGraph::with_integer_edges(3, &[(0, 1, 1), (1, 2, 1)]);
        let mut search = Search::new(&space);
        let flag = AtomicBool::new(true);
        let limits = SearchLimits {
            max_expansions: u64::MAX,
            cancel: Some(&flag),
        };

        let outcome =
            search.find_path_with(&space, NET, sources(&[0]), with_costs(&[(2, 0)]), limits);
        assert_eq!(outcome_summary(outcome), Err(SearchEnd::Cancelled));
        assert_eq!(space.expansions.get(), 0);

        flag.store(false, Ordering::Relaxed);
        let outcome =
            search.find_path_with(&space, NET, sources(&[0]), with_costs(&[(2, 0)]), limits);
        assert_eq!(outcome_summary(outcome), Ok((vec![0, 1, 2], 2.0)));
    }

    #[test]
    fn a_flag_set_during_a_long_search_is_seen_within_the_interval() {
        /// A row of nodes that raises a flag when it reaches a given one.
        struct Trip<'a> {
            flag: &'a AtomicBool,
            at: u32,
        }
        impl RoutingSpace for Trip<'_> {
            type Node = NodeId;
            type Resource = NodeId;
            type Slots = DenseSlots<NodeId>;
            type ResourceSlots = DenseSlots<NodeId>;
            fn new_slots(&self) -> Self::Slots {
                DenseSlots::new()
            }
            fn new_resource_slots(&self) -> Self::ResourceSlots {
                DenseSlots::new()
            }
            fn contains(&self, node: NodeId) -> bool {
                node.0 < u32::MAX
            }
            fn expand(&self, _net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
                if from.0 == self.at {
                    self.flag.store(true, Ordering::Relaxed);
                }
                out.push(Step {
                    to: NodeId(from.0 + 1),
                    cost: Cost::ONE,
                });
            }
            fn lower_bound(&self, _net: NetId, _from: NodeId, _to: NodeId) -> Cost {
                Cost::ZERO
            }
            fn resources(&self, _: NetId, _: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
                out.push(to);
            }
            fn capacity(&self, _resource: NodeId) -> u32 {
                1
            }
        }

        let flag = AtomicBool::new(false);
        let space = Trip {
            flag: &flag,
            at: 100,
        };
        let mut search = Search::new(&space);
        let limits = SearchLimits {
            max_expansions: u64::MAX,
            cancel: Some(&flag),
        };
        let outcome = search.find_path_with(
            &space,
            NET,
            sources(&[0]),
            with_costs(&[(100_000, 0)]),
            limits,
        );
        assert_eq!(outcome_summary(outcome), Err(SearchEnd::Cancelled));
        assert_eq!(search.stats().expanded, CANCEL_CHECK_INTERVAL);
    }

    #[test]
    fn a_wrapper_space_can_share_a_search_with_the_space_it_wraps() {
        /// Doubles every step, and borrows what it wraps for a short time only.
        struct Doubled<'a>(&'a TestGraph);
        impl RoutingSpace for Doubled<'_> {
            type Node = NodeId;
            type Resource = NodeId;
            type Slots = DenseSlots<NodeId>;
            type ResourceSlots = DenseSlots<NodeId>;
            fn new_slots(&self) -> Self::Slots {
                self.0.new_slots()
            }
            fn new_resource_slots(&self) -> Self::ResourceSlots {
                DenseSlots::new()
            }
            fn contains(&self, node: NodeId) -> bool {
                self.0.contains(node)
            }
            fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
                self.0.expand(net, from, out);
                for step in out.iter_mut() {
                    step.cost = step.cost * Cost::from(2);
                }
            }
            fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
                self.0.lower_bound(net, from, to)
            }
            fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
                self.0.resources(net, from, to, out);
            }
            fn capacity(&self, resource: NodeId) -> u32 {
                self.0.capacity(resource)
            }
        }

        let space = TestGraph::with_integer_edges(2, &[(0, 1, 3)]);
        let mut search = Search::new(&space);
        for _ in 0..2 {
            let wrapped = Doubled(&space);
            let outcome = search.find_path_with(
                &wrapped,
                NET,
                sources(&[0]),
                with_costs(&[(1, 1)]),
                SearchLimits::NONE,
            );
            assert_eq!(outcome_summary(outcome), Ok((vec![0, 1], 7.0)));
        }
    }

    proptest! {
        #[test]
        fn heap_pops_in_sorted_order(
            keys in prop::collection::vec((0..8u64, 0..8u64), 0..200),
            pops_between in prop::collection::vec(0..3usize, 0..200),
        ) {
            let mut heap = QuadHeap::default();
            let mut model: Vec<HeapEntry> = Vec::new();
            let mut pops = pops_between.iter().copied().cycle();

            for (sequence, &(f, g)) in keys.iter().enumerate() {
                let new = entry(f, g, sequence as u64);
                heap.push(new);
                model.push(new);
                for _ in 0..pops.next().unwrap_or(0) {
                    prop_assert_eq!(heap.pop(), pop_first(&mut model));
                }
            }
            while !model.is_empty() {
                prop_assert_eq!(heap.pop(), pop_first(&mut model));
            }
            prop_assert_eq!(heap.pop(), None);
        }

        #[test]
        fn integer_costs_match_brute_force_exactly(case in case()) {
            let starts = integer_starts(&case);
            let expected = brute_force_cost(
                &TestGraph::with_integer_edges(case.nodes, &case.edges),
                starts.iter().copied(),
                &case.targets,
            );

            // Dijkstra, then A* with an admissible but inconsistent bound.
            for space in [
                TestGraph::with_integer_edges(case.nodes, &case.edges),
                with_integer_bound(&case),
            ] {
                let mut search = Search::new(&space);
                let found = search.find_path(
                    &space,
                    NET,
                    cost_sources(&starts),
                    targets(&case.targets),
                );
                prop_assert_eq!(found.map(|found| found.cost), expected);
                if let Some(found) = found {
                    let cost = path_cost(&space, &starts, &case.targets, found.path)?;
                    prop_assert_eq!(cost, found.cost);
                }
            }
        }

        #[test]
        fn inadmissible_bounds_give_valid_paths_of_the_reported_cost(case in case()) {
            let starts = integer_starts(&case);
            let space = TestGraph::with_integer_edges(case.nodes, &case.edges).with_bound(
                |from, to| Cost::from(u32::from(case.fractions[from * case.nodes + to])),
            );
            let expected = brute_force_cost(&space, starts.iter().copied(), &case.targets);

            let mut search = Search::new(&space);
            let found =
                search.find_path(&space, NET, cost_sources(&starts), targets(&case.targets));
            prop_assert_eq!(found.is_some(), expected.is_some());
            if let (Some(found), Some(expected)) = (found, expected) {
                prop_assert!(found.cost >= expected);
                let cost = path_cost(&space, &starts, &case.targets, found.path)?;
                prop_assert_eq!(cost, found.cost);
            }
        }

        #[test]
        fn rounded_dijkstra_matches_brute_force_exactly(case in case(), scale in 1e-6..1e6f64) {
            let starts = integer_starts(&case);
            let mut space = with_fractional_costs(&case, scale);
            space.bounds.clear();
            let expected = brute_force_cost(&space, starts.iter().copied(), &case.targets);

            let mut search = Search::new(&space);
            let found =
                search.find_path(&space, NET, cost_sources(&starts), targets(&case.targets));
            prop_assert_eq!(found.map(|found| found.cost), expected);
            if let Some(found) = found {
                let cost = path_cost(&space, &starts, &case.targets, found.path)?;
                prop_assert_eq!(cost, found.cost);
            }
        }

        #[test]
        fn rounded_a_star_is_within_the_error_bound(case in case(), scale in 1e-6..1e6f64) {
            let starts = integer_starts(&case);
            let space = with_fractional_costs(&case, scale);
            let expected = brute_force_cost(&space, starts.iter().copied(), &case.targets);

            let mut search = Search::new(&space);
            let found =
                search.find_path(&space, NET, cost_sources(&starts), targets(&case.targets));
            prop_assert_eq!(found.is_some(), expected.is_some());
            if let (Some(found), Some(expected)) = (found, expected) {
                prop_assert!(close(found.cost, expected), "{:?} {:?}", found.cost, expected);
                // A path can cost less than reported, never more.
                let cost = path_cost(&space, &starts, &case.targets, found.path)?;
                prop_assert!(cost <= found.cost);
                prop_assert!(close(cost, found.cost));
            }
        }

        #[test]
        fn costs_near_the_maximum_match_brute_force(case in case()) {
            // Stretches the small numbers across the range of magnitudes, including ones
            // that saturate when added.
            let stretch = |cost: u32| match cost % 5 {
                0 => Cost::ZERO,
                1 => Cost::ONE,
                2 => Cost::new(f64::MAX / 3.0).unwrap(),
                3 => Cost::new(f64::MAX / 2.0).unwrap(),
                _ => Cost::MAX,
            };
            let edges: Vec<_> =
                case.edges.iter().map(|&(a, b, cost)| (a, b, stretch(cost))).collect();
            let starts: Vec<_> =
                case.sources.iter().map(|&(node, cost)| (node, stretch(cost))).collect();

            let space = TestGraph::new(case.nodes, &edges);
            let expected = brute_force_cost(&space, starts.iter().copied(), &case.targets);
            let mut search = Search::new(&space);
            let found =
                search.find_path(&space, NET, cost_sources(&starts), targets(&case.targets));
            prop_assert_eq!(found.map(|found| found.cost), expected);
            if let Some(found) = found {
                let cost = path_cost(&space, &starts, &case.targets, found.path)?;
                prop_assert_eq!(cost, found.cost);
            }
        }

        /// Target costs against brute force, where each target's cost is added to its
        /// distance. Every bound is admissible, inconsistent, and blind to the target costs.
        #[test]
        fn target_costs_match_brute_force(
            case in case(),
            finishing in prop::collection::vec(0..=20u32, 4),
        ) {
            let starts = integer_starts(&case);
            let goals: Vec<(u32, u32)> = case
                .targets
                .iter()
                .zip(&finishing)
                .map(|(&node, &cost)| (node, cost))
                .collect();
            let plain = TestGraph::with_integer_edges(case.nodes, &case.edges);
            let distances = bellman_ford(&plain, starts.iter().copied());
            let expected = goals
                .iter()
                .filter_map(|&(node, cost)| {
                    distances[node as usize].map(|distance| distance + Cost::from(cost))
                })
                .min();

            for space in [plain, with_integer_bound(&case)] {
                let mut search = Search::new(&space);
                let outcome = search.find_path_with(
                    &space,
                    NET,
                    cost_sources(&starts),
                    with_costs(&goals),
                    SearchLimits::NONE,
                );
                match outcome {
                    SearchOutcome::Found(found) => {
                        prop_assert_eq!(Some(found.cost), expected);
                        let nodes: Vec<u32> = goals.iter().map(|goal| goal.0).collect();
                        let cost = path_cost(&space, &starts, &nodes, found.path)?;
                        // The finish of this target, at the lowest cost given for it.
                        let last = found.path.last().unwrap().0;
                        let finish = goals
                            .iter()
                            .filter(|goal| goal.0 == last)
                            .map(|goal| Cost::from(goal.1))
                            .min()
                            .unwrap();
                        prop_assert_eq!(cost + finish, found.cost);
                    }
                    SearchOutcome::Unreachable => prop_assert_eq!(expected, None),
                    other => prop_assert!(false, "no limit was set, got {:?}", other),
                }
            }
        }

        /// Limits only ever stop a search early. A search allowed fewer expansions than it
        /// needed must say so, and with enough it must behave as if it had no limit.
        #[test]
        fn limits_never_change_an_answer(case in case(), fraction in 0.0..1.0f64) {
            let starts = integer_starts(&case);
            let space = TestGraph::with_integer_edges(case.nodes, &case.edges);
            let goals: Vec<_> = case.targets.iter().map(|&node| (node, 0)).collect();
            let run = |search: &mut Search<TestGraph>, max_expansions| {
                let limits = SearchLimits { max_expansions, cancel: None };
                outcome_summary(search.find_path_with(
                    &space,
                    NET,
                    cost_sources(&starts),
                    with_costs(&goals),
                    limits,
                ))
            };

            let mut search = Search::new(&space);
            let unlimited = run(&mut search, u64::MAX);
            let needed = search.stats().expanded;
            prop_assert_eq!(run(&mut search, needed), unlimited);
            prop_assert_eq!(search.stats().expanded, needed);
            if needed > 0 {
                let fewer = (needed as f64 * fraction) as u64;
                prop_assert_eq!(run(&mut search, fewer), Err(SearchEnd::LimitReached));
                prop_assert_eq!(search.stats().expanded, fewer);
            }
        }

        #[test]
        fn a_bound_of_the_maximum_changes_nothing_but_pushes(
            case in case(),
            maximal in prop::collection::vec(any::<bool>(), 144),
            weight in prop::sample::select(vec![1.0, 1.5]),
        ) {
            // Offers of nodes whose bound is the maximum are held back, which must not change
            // the order anything leaves the queue in. Against a bound one step below it, which
            // sorts the same but is never held back, on the same reused searches, with bounds
            // of the maximum both where there is no way and, inadmissibly, where there is.
            let plain = TestGraph::with_integer_edges(case.nodes, &case.edges);
            let distances: Vec<_> = (0..case.nodes as u32)
                .map(|from| bellman_ford(&plain, [(from, Cost::ZERO)]))
                .collect();
            let below = Cost::new(f64::MAX.next_down()).unwrap();
            let (distances, maximal, fractions, nodes) =
                (&distances, &maximal, &case.fractions, case.nodes);
            let bound = |top: Cost| {
                move |from: usize, to: usize| match distances[from][to] {
                    _ if maximal[from * 12 + to] => top,
                    None => top,
                    Some(distance) => {
                        let fraction = u32::from(fractions[from * nodes + to]);
                        Cost::from((distance.get() as u32) * fraction / 255)
                    }
                }
            };
            let held = TestGraph::with_integer_edges(case.nodes, &case.edges)
                .with_bound(bound(Cost::MAX));
            let queued = TestGraph::with_integer_edges(case.nodes, &case.edges)
                .with_bound(bound(below));
            let starts = integer_starts(&case);
            let mut on_held = Search::new(&held);
            let mut on_queued = Search::new(&queued);
            on_held.set_bound_weight(weight);
            on_queued.set_bound_weight(weight);
            for _ in 0..2 {
                let a = on_held
                    .find_path(&held, NET, cost_sources(&starts), targets(&case.targets))
                    .map(|found| (found.path.to_vec(), found.cost));
                let b = on_queued
                    .find_path(&queued, NET, cost_sources(&starts), targets(&case.targets))
                    .map(|found| (found.path.to_vec(), found.cost));
                prop_assert_eq!(a, b);
                let (a, b) = (on_held.stats(), on_queued.stats());
                prop_assert_eq!(a.expanded, b.expanded);
                prop_assert_eq!(a.stale_pops, b.stale_pops);
                prop_assert!(a.pushes <= b.pushes);
            }
        }

        #[test]
        fn reused_searches_match_new_ones_exactly(cases in prop::collection::vec(case(), 1..6)) {
            // A `Search` serves one space, so the cases become parts of one large space.
            let nodes = cases.iter().map(|case| case.nodes).sum::<usize>();
            let mut edges = Vec::new();
            let mut offsets = Vec::new();
            let mut offset = 0;
            for case in &cases {
                offsets.push(offset);
                edges.extend(case.edges.iter().map(|&(from, to, cost)| {
                    (from + offset, to + offset, cost)
                }));
                offset += case.nodes as u32;
            }
            let space = TestGraph::with_integer_edges(nodes, &edges);

            let mut reused = Search::new(&space);
            // Twice through, in case the first pass leaves something for the second.
            for _ in 0..2 {
                for (case, &offset) in cases.iter().zip(&offsets) {
                    let starts: Vec<_> = integer_starts(case)
                        .into_iter()
                        .map(|(node, cost)| (node + offset, cost))
                        .collect();
                    let goals: Vec<_> = case.targets.iter().map(|&node| node + offset).collect();

                    let mut fresh = Search::new(&space);
                    let expected = fresh
                        .find_path(&space, NET, cost_sources(&starts), targets(&goals))
                        .map(|found| (found.path.to_vec(), found.cost));
                    let actual = reused
                        .find_path(&space, NET, cost_sources(&starts), targets(&goals))
                        .map(|found| (found.path.to_vec(), found.cost));
                    prop_assert_eq!(actual, expected);
                    prop_assert_eq!(reused.stats(), fresh.stats());
                }
            }
        }
    }

    /// One search of a run that reuses bounds: its sources, and its targets with their costs.
    type Round = (Vec<(u32, u32)>, Vec<(u32, u32)>);

    /// Up to four searches over one graph, whose targets mostly shrink from one to the next,
    /// as a tree's do, but not always.
    fn rounds() -> impl Strategy<Value = (Case, Vec<Round>, u8)> {
        (case(), 0..4u8).prop_flat_map(|(case, weight)| {
            let node = 0..case.nodes as u32;
            let first = prop::collection::vec((node.clone(), 0..=3u32), 1..8);
            let later = (
                prop::collection::vec((node.clone(), 0..=20u32), 0..4),
                prop::collection::vec(any::<bool>(), 8),
                prop::option::weighted(0.2, prop::collection::vec((node.clone(), 0..=3u32), 0..4)),
            );
            (
                Just(case),
                first,
                prop::collection::vec(later, 0..4),
                Just(weight),
            )
                .prop_map(|(case, first, later, weight)| {
                    let mut rounds = vec![(integer_starts_of(&case), first)];
                    for (sources, keep, other) in later {
                        let previous = &rounds[rounds.len() - 1].1;
                        let targets = other.unwrap_or_else(|| {
                            previous
                                .iter()
                                .zip(keep.iter().cycle())
                                .filter(|&(_, &keep)| keep)
                                .map(|(&target, _)| target)
                                .collect()
                        });
                        rounds.push((sources, targets));
                    }
                    (case, rounds, weight)
                })
        })
    }

    fn integer_starts_of(case: &Case) -> Vec<(u32, u32)> {
        case.sources.clone()
    }

    /// A graph whose slots are given out in the order nodes come up, anew in each search.
    struct Reslotted<'a>(&'a TestGraph);

    impl RoutingSpace for Reslotted<'_> {
        type Node = NodeId;
        type Resource = NodeId;
        type Slots = InternedSlots<NodeId>;
        type ResourceSlots = InternedSlots<NodeId>;

        fn new_slots(&self) -> Self::Slots {
            InternedSlots::new()
        }

        fn new_resource_slots(&self) -> Self::ResourceSlots {
            InternedSlots::new()
        }

        fn contains(&self, node: NodeId) -> bool {
            self.0.contains(node)
        }

        fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
            self.0.expand(net, from, out);
        }

        fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
            self.0.lower_bound(net, from, to)
        }

        fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
            self.0.resources(net, from, to, out);
        }

        fn capacity(&self, resource: NodeId) -> u32 {
            self.0.capacity(resource)
        }
    }

    /// Runs `rounds` with one search that reuses bounds and checks each against a new one.
    fn check_reuse<T: RoutingSpace<Node = NodeId>>(
        space: &T,
        rounds: &[Round],
        weight: f64,
    ) -> Result<(), TestCaseError> {
        let mut reusing = Search::new(space);
        reusing.set_bound_weight(weight);
        for (sources, targets) in rounds {
            let sources: Vec<_> = sources
                .iter()
                .map(|&(node, cost)| Source::with_cost(NodeId(node), Cost::from(cost)))
                .collect();
            let targets: Vec<_> = targets
                .iter()
                .map(|&(node, cost)| Target::with_cost(NodeId(node), Cost::from(cost)))
                .collect();
            let mut fresh = Search::new(space);
            fresh.set_bound_weight(weight);
            let expected = outcome_summary(fresh.find_path_with(
                space,
                NET,
                sources.iter().copied(),
                targets.iter().copied(),
                SearchLimits::default(),
            ));
            reusing.reuse_bounds();
            let found = outcome_summary(reusing.find_path_with(
                space,
                NET,
                sources.iter().copied(),
                targets.iter().copied(),
                SearchLimits::default(),
            ));
            prop_assert_eq!(found, expected);
            prop_assert_eq!(reusing.stats(), fresh.stats());
        }
        Ok(())
    }

    proptest! {
        // Many cases, as it takes a tie between a target that goes and one that stays to tell
        // a test that is almost right from one that is.
        #![proptest_config(ProptestConfig::with_cases(8192))]

        /// Reusing heuristics changes nothing about any search, including its statistics,
        /// whatever its targets, weight and bounds, which here include the maximum.
        #[test]
        fn reused_bounds_change_nothing((case, rounds, weight) in rounds()) {
            let space = TestGraph::with_integer_edges(case.nodes, &case.edges).with_bound(
                |from, to| match case.fractions[from * case.nodes + to] {
                    255 => Cost::MAX,
                    fraction => Cost::from(u32::from(fraction) / 16),
                },
            );
            let weight = [1.0, 1.1, 0.7, 2.0][usize::from(weight)];
            check_reuse(&space, &rounds, weight)?;
            // Slots handed out afresh for each search, so that one goes to another node.
            check_reuse(&Reslotted(&space), &rounds, weight)?;
        }
    }
}
