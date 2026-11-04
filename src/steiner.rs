//! Steiner tree construction for multi-terminal nets.
//!
//! Given one net, [`kmb_tree`] builds a [`Route`] rooted at an access point of the net's
//! driver, with every edge a step that [`RoutingSpace::expand`] offers in that direction.
//! [`takahashi_matsuyama`] is the simple baseline, and [`refine_tree`] improves a route
//! from anywhere. All of them work over any [`RoutingSpace`], and take costs from the
//! space, so a congestion-priced space gets trees priced for that congestion.
//!
//! # What is minimized
//!
//! The cost of a tree is the sum of its step costs, plus the access cost of the driver at
//! the root, plus that of every other terminal at its cheapest access point on the tree.
//! With free access points, which is what [`Terminal::at`](crate::net::Terminal::at)
//! gives, that is just the sum of the step costs.
//!
//! # Construction
//!
//! KMB (Kou, Markowsky and Berman 1981) takes shortest paths between terminals, a minimum
//! spanning tree over those distances, and the union of the paths reduced to a tree. That
//! leans on symmetry twice: a spanning tree has no direction, and it can be rooted
//! anywhere afterwards. A directed space allows neither, so [`kmb_tree`] does this:
//!
//! 1. **Pick the root.** Grow a tree by [`takahashi_matsuyama`] from each access point of
//!    the driver, and keep the cheapest. This also settles whether the net can be connected
//!    at all, and the tree it leaves is the one to beat.
//! 2. **Closure.** From each terminal, search for the nearest other terminals, one search
//!    per closure edge. Edges are directed, and none enters the driver.
//! 3. **Arborescence.** Take a minimum arborescence of the closure, rooted at the driver,
//!    by the Chu-Liu/Edmonds algorithm. The cheapest edge into each terminal can form cycles
//!    that the driver does not reach, which is the case that a greedy choice gets wrong.
//! 4. **Expansion.** Walk the arborescence depth first, and search from the *whole tree so
//!    far* to each terminal in turn.
//! 5. **Cleanup.** Remove branches that serve no terminal, keep the cheaper of this tree and
//!    the one from step 1, and refine it.
//!
//! Step 4 differs from the textbook, which keeps each closure path and reduces their union,
//! for example to a shortest-path tree from the driver. Searching again costs `k - 1` more
//! searches, needs no stored paths, and can only do better on the closure's edge, since the
//! parent is on the tree. It is also better in practice: the textbook expansion was worse
//! than plain growth on average on grids, and only slightly better on directed graphs (see
//! [the measurements](#measured-quality)). Reducing the union by shortest paths or by a
//! minimum arborescence gave identical costs on all 100 000 nets measured, as the paths of
//! a minimum arborescence can only converge where their distances tie.
//!
//! # Guarantees
//!
//! Let `k` be the number of terminals. These two hold unconditionally:
//!
//! - A tree is returned exactly when one access point of the driver reaches some access
//!   point of every other terminal, and it passes [`Route::validate`], is rooted at the
//!   driver, follows `expand`, and has no leaf that serves no terminal.
//! - [`kmb_tree`] never costs more than [`takahashi_matsuyama`] on the same input, and
//!   [`refine_tree`] never costs more than the route it was given.
//!
//! The bounds against the optimum `OPT` assume that sums do not round and that the space's
//! lower bound is admissible (see [the search module](crate::search#optimality)):
//!
//! | Net and space | Bound |
//! | --- | --- |
//! | Two terminals, any space and access points | Exactly `OPT` |
//! | Any net in any space | `(k - 1) * OPT` |
//! | One free access point per terminal | The weight of the arborescence over the closure that was built |
//! | The same, in a symmetric space (each step has a reverse of equal cost) | `2 * (1 - 1 / k) * OPT` |
//!
//! **The factor of 2 does not survive direction.** It comes from walking around the optimal
//! tree and over each edge twice, and in a directed space those reverse edges need not
//! exist. What is left is that each terminal is at most `OPT` from the root, which gives
//! `k - 1`. That is close to tight for this method: give each of `n` sinks a private hub
//! costing 9, and let all of them share a hub costing 10. Every shortest path avoids the
//! shared hub, and so does every reconnection of one sink, so the result is `9 * n` against
//! an optimum of 10. A test builds this. Nothing cheap can promise much better, since
//! directed Steiner tree is at least as hard as set cover, and the best known ratios
//! (Charikar et al. 1999) need far more time than this.
//!
//! # Access points
//!
//! Treating a terminal as a group of equivalent nodes makes this the *group* Steiner tree
//! problem, which is hard to approximate even when the space is a tree: under standard
//! assumptions, no ratio of `log^(2 - e) k` is possible for any `e > 0` (Halperin and
//! Krauthgamer 2003). Only the `k - 1` bound applies to groups, symmetric or not. In practice:
//!
//! - Closure searches run between whole groups, so each edge is the cheapest path between
//!   any access point of one terminal and any of the other.
//! - An arborescence cannot say which access point to use. It may count on arriving at a
//!   terminal at one access point and leaving from another, and those are not connected
//!   for free in a route. So when a net has any such choices, step 4 runs twice: once
//!   arriving at the nearest access point, and once arriving where the terminal's cheapest
//!   edge out starts. On nets with up to 3 access points per terminal, this took the mean
//!   excess over the optimum from about 1.4% to 0.8%. The test named
//!   `the_access_point_to_arrive_at_is_the_one_that_leads_on` shows why.
//! - A route has one root, so the driver's access points are tried as whole trees in step 1.
//!   Later steps stay with the chosen root.
//! - Cleanup removes a branch to one access point if another of the same terminal is on the
//!   tree and doing so saves more than the difference in their access costs.
//!
//! # Refinement
//!
//! A *key path* runs up from a node holding a terminal, or where the tree branches, to the
//! nearest node above that also needs to stay. Refinement removes one along with everything
//! below it, reconnects the terminals that lost out, nearest first, and keeps the result if
//! it is strictly cheaper. Undirected key-path exchange reconnects the two parts at any
//! nodes, but in a directed space the lower part can only be entered at its root, so it is
//! rebuilt instead. Key paths nearest the leaves go first, since they cost the fewest
//! searches. On the measurements below it does more for the cost than the closure does.
//!
//! # Cost
//!
//! [`Search::find_path`] stops at its first target, so a closure takes one search per edge
//! and not one per terminal. With `d` distinct access points on the driver, `l` distinct
//! access costs among a search's targets (nearly always 1, as those need a search each), `m`
//! for [`closure_neighbours`](SteinerOptions::closure_neighbours) and `r` for
//! [`refine_searches_per_terminal`](SteinerOptions::refine_searches_per_terminal), the
//! number of searches is at most:
//!
//! | Step | Searches |
//! | --- | --- |
//! | Root and growth | `d * (k - 1) * l` |
//! | Closure | `(k - 1) * (min(m, k - 2) + 2) * l` |
//! | Expansion | `(k - 1) * l`, or `3 * (k - 1) * l` if a terminal has several access points |
//! | Refinement | `r * (k - 1) + (k - 1) * l`, as the last exchange can run over the budget |
//!
//! So with one free access point per terminal the defaults come to at most `21 * (k - 1)`
//! searches, and the number per terminal does not grow with the size of the net. Asking for
//! the full closure gives `k^2`, which has not paid for itself on any net measured, and growth
//! alone takes `k - 1`. Each search costs what [`Search::find_path`] does
//! with up to the whole tree as sources, and its heuristic takes one
//! [`lower_bound`](RoutingSpace::lower_bound) per target for each node reached.
//!
//! Outside the searches, the arborescence takes `O(m * k^2)` at worst, and each exchange
//! takes time linear in the size of the tree, times one [`expand`](RoutingSpace::expand) per
//! edge to read its cost. Only the [`Search`] is reused between calls, while this module's
//! own bookkeeping is allocated per call. That is small next to the searches, and no paths
//! are kept between them.
//!
//! # Determinism
//!
//! The same input gives the same tree, with the same counters, in a new or reused
//! [`Search`]. Hash maps are only used for lookups. On top of [the search's own
//! rules](crate::search#determinism), ties are broken as follows:
//!
//! - Among equally cheap access costs and terminals, the search decides. Among terminals
//!   sharing the node it reached, the lowest index wins.
//! - Among driver access points giving equally cheap trees, the first listed wins.
//! - Closure edges are ordered by `(from, to)`, and the earliest wins among equal reduced
//!   weights. Siblings connect in order of closure weight, then terminal index.
//! - A later candidate replaces a tree only if it is strictly cheaper.
//!
//! All arithmetic is addition and subtraction, in a fixed order.
//!
//! # Measured quality
//!
//! Each row is 20 000 random nets of 3 to 6 terminals on spaces of 8 to 72 nodes, against an
//! exact optimum by dynamic programming over subsets, and shows the mean and worst ratio
//! to it. Grids are 4 to 6 cells a side with 15% blocked. "Graph" means `2n` to `4n` random
//! edges of cost 1 to 20, "fabric" means a layered directed graph, and the last three rows
//! are nets with up to 3 access points per terminal.
//!
//! | Family | Growth | Textbook KMB | Growth, then [`refine_tree`] | [`kmb_tree`], no refinement | [`kmb_tree`] |
//! | --- | --- | --- | --- | --- | --- |
//! | Grid | 1.024, 1.43 | 1.033, 1.43 | 1.009, 1.33 | 1.018, 1.33 | 1.007, 1.33 |
//! | 2-layer grid | 1.026, 1.40 | 1.053, 1.42 | 1.014, 1.33 | 1.021, 1.33 | 1.013, 1.33 |
//! | Symmetric graph | 1.008, 1.33 | 1.015, 1.36 | 1.006, 1.29 | 1.007, 1.33 | 1.005, 1.29 |
//! | Directed graph | 1.042, 2.25 | 1.027, 1.81 | 1.012, 1.93 | 1.010, 1.59 | 1.004, 1.47 |
//! | Directed fabric | 1.035, 2.00 | 1.023, 1.79 | 1.004, 1.53 | 1.015, 1.79 | 1.004, 1.53 |
//! | Grid, groups | 1.031, 2.00 | | 1.015, 2.00 | 1.015, 2.00 | 1.009, 2.00 |
//! | Directed graph, groups | 1.039, 2.56 | | 1.017, 2.38 | 1.015, 2.00 | 1.008, 1.88 |
//! | The same, priced access points | 1.031, 2.32 | | 1.014, 2.32 | 1.011, 1.71 | 1.006, 1.68 |
//!
//! Against growth on larger nets, where no optimum is available, with the default options.
//! "Latest arrival" is the cost along the tree from the root to the farthest terminal, as
//! a stand-in for delay, since a tree that spends less overall can reach further to do it.
//! These samples are small, at 200 nets for the smallest and 5 for the largest, so the
//! figures are indicative.
//!
//! | Family | Terminals | Cost | Latest arrival | Searches | Time |
//! | --- | --- | --- | --- | --- | --- |
//! | 32 x 32 grid | 5 | 0.990 | 1.00 | 9 x | 8 x |
//! | 64 x 64 grid | 25 | 0.989 | 1.02 | 19 x | 21 x |
//! | 128 x 128 grid | 100 | 0.993 | 1.00 | 19 x | 12 x |
//! | Directed graph of 500 | 5 | 0.953 | 1.02 | 10 x | 7 x |
//! | Directed graph of 2000 | 25 | 0.922 | 1.12 | 19 x | 15 x |
//! | Directed graph of 10 000 | 100 | 0.932 | 1.10 | 19 x | 15 x |
//!
//! `refine_tree` on growth's tree instead gets most of that for less: 0.989 to 0.993 on the
//! grids and 0.930 to 0.972 on the directed graphs, at 5 to 13 times the time of growth, with
//! latest arrival up by as much as 10%, against as much as 15% for [`kmb_tree`] on the sizes
//! not shown here. Refinement and the closure both pay for themselves most on directed
//! spaces, where nearest-first growth goes wrong most often. None of this was measured under
//! negotiated congestion.
//!
//! To reproduce all of it:
//!
//! ```text
//! cargo test --release --lib steiner::tests::measure -- --ignored --nocapture --test-threads=1
//! ```
//!
//! # Examples
//!
//! Three terminals join in a T, at a point that is none of them. Connecting each one
//! directly to the driver would cost 4 + 5 = 9.
//!
//! ```
//! use guide::id::NetId;
//! use guide::net::{Net, Terminal};
//! use guide::search::Search;
//! use guide::space::grid::Grid;
//! use guide::steiner::{SteinerOptions, kmb_tree};
//!
//! let grid = Grid::new(5, 4, 1)?;
//! let at = |x, y| grid.node_at(x, y, 0).unwrap();
//! let net = Net {
//!     id: NetId(0),
//!     terminals: vec![
//!         Terminal::at(at(0, 0)), // The driver.
//!         Terminal::at(at(4, 0)),
//!         Terminal::at(at(2, 3)),
//!     ],
//! };
//!
//! // Keep the search and reuse it, so that its buffers are only allocated once.
//! let mut search = Search::new(&grid);
//! let tree = kmb_tree(&mut search, &grid, &net, &SteinerOptions::default()).unwrap();
//!
//! assert_eq!(tree.cost.get(), 7.0);
//! assert_eq!(tree.route.root(), Some(at(0, 0)));
//! assert!(tree.route.contains(at(2, 0)));
//! assert_eq!(tree.route.validate(&net), Ok(()));
//! # Ok::<(), guide::space::grid::GridError>(())
//! ```
//!
//! # References
//!
//! - H. Takahashi, A. Matsuyama, "An Approximate Solution for the Steiner Problem in
//!   Graphs", *Math. Japonica*, 1980.
//! - L. Kou, G. Markowsky, L. Berman, "A Fast Algorithm for Steiner Trees", *Acta
//!   Informatica*, 1981.
//! - Y. Chu, T. Liu, "On the Shortest Arborescence of a Directed Graph", *Scientia Sinica*,
//!   1965, and J. Edmonds, "Optimum Branchings", *J. Res. Nat. Bur. Standards*, 1967.
//! - M. Charikar et al., "Approximation Algorithms for Directed Steiner Problems",
//!   *J. Algorithms*, 1999.
//! - E. Halperin, R. Krauthgamer, "Polylogarithmic Inapproximability", *STOC*, 2003.

use core::fmt;
use std::collections::{HashMap, HashSet};
use std::error::Error;

use crate::cost::Cost;
use crate::net::{Edge, Net, Route, RouteError};
use crate::search::{Search, Source};
use crate::space::{RoutingSpace, Step};

/// Bounds how much work [`kmb_tree`] and [`refine_tree`] do on one net.
///
/// See the [module documentation](self#cost) for what these come to. Create this with
/// [`Default`] and set the fields you want to change:
///
/// ```
/// use guide::steiner::SteinerOptions;
///
/// let mut options = SteinerOptions::default();
/// options.refine_searches_per_terminal = 0;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SteinerOptions {
    /// How many other terminals each terminal but the driver searches for in the closure,
    /// nearest first. The driver always searches for all of them, so that an arborescence
    /// always exists. [`usize::MAX`] asks for the full closure.
    ///
    /// The default is 8, which came within sampling noise of the full closure at a fraction
    /// of its cost on every net measured.
    pub closure_neighbours: usize,
    /// How many searches refinement may start, per terminal other than the driver. Zero
    /// turns refinement off, though [`refine_tree`] still cleans up its input.
    ///
    /// The default is 8, past which little was gained, except that 16 still found a little
    /// on nets of 50 terminals or more.
    pub refine_searches_per_terminal: usize,
}

impl Default for SteinerOptions {
    fn default() -> Self {
        SteinerOptions {
            closure_neighbours: 8,
            refine_searches_per_terminal: 8,
        }
    }
}

/// A tree connecting a net, what it costs, and what it took to find.
///
/// A net that does not [need routing](Net::needs_routing) gets an empty route at no cost.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SteinerTree<N> {
    /// The tree, rooted at an access point of the net's driver.
    pub route: Route<N>,
    /// The total: [`step_cost`](SteinerTree::step_cost) plus
    /// [`access_cost`](SteinerTree::access_cost).
    pub cost: Cost,
    /// The costs of the route's steps as the space reports them, added up in edge order.
    pub step_cost: Cost,
    /// The access cost of the driver at the root, plus that of every other terminal at
    /// its cheapest access point on the route.
    pub access_cost: Cost,
    /// How many searches this call ran.
    pub searches: u64,
    /// How many nodes those searches expanded.
    pub nodes_expanded: u64,
}

/// Why a net has no tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SteinerError {
    /// No route from the driver can reach this terminal along with all the others.
    ///
    /// With several access points on the driver, every one of them failed, possibly on
    /// different terminals, since a route has just one root. This then names the lowest
    /// terminal that the most successful access point could not reach, with the first listed
    /// winning a tie between them. So fixing this terminal alone may not be enough.
    UnreachableTerminal {
        /// The terminal's index in [`Net::terminals`], which is never 0.
        terminal: usize,
    },
}

impl fmt::Display for SteinerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SteinerError::UnreachableTerminal { terminal } => {
                write!(f, "terminal {terminal} cannot be reached from the driver")
            }
        }
    }
}

impl Error for SteinerError {}

/// Why [`refine_tree`] rejected a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RefineError<N> {
    /// The route is not a tree that reaches every terminal.
    InvalidRoute(RouteError<N>),
    /// The route's root is not an access point of the net's driver.
    RootNotDriver {
        /// The route's root.
        root: N,
    },
    /// An edge of the route is not a step that the space offers.
    NotAStep {
        /// The start of the edge.
        from: N,
        /// The end of the edge.
        to: N,
    },
}

impl<N: fmt::Debug> fmt::Display for RefineError<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefineError::InvalidRoute(error) => write!(f, "invalid route: {error}"),
            RefineError::RootNotDriver { root } => {
                write!(
                    f,
                    "route's root {root:?} is not an access point of the driver"
                )
            }
            RefineError::NotAStep { from, to } => {
                write!(f, "the space has no step from {from:?} to {to:?}")
            }
        }
    }
}

impl<N: fmt::Debug> Error for RefineError<N> {}

/// Builds a tree for `net` from a closure over its terminals, as the
/// [module documentation](self#construction) describes.
///
/// It never costs more than [`takahashi_matsuyama`] on the same input, and takes many
/// times as long, so see [the measurements](self#measured-quality) before using it for every
/// net of every iteration.
///
/// `search` must belong to `space`, as for [`Search::find_path`]. See
/// [the module documentation](self#examples) for an example.
///
/// # Errors
///
/// Returns [`SteinerError::UnreachableTerminal`] if no access point of the driver reaches
/// all the other terminals.
///
/// # Panics
///
/// Panics if the space breaks the promise of [`RoutingSpace`] by no longer offering a step
/// that it offered during a search.
pub fn kmb_tree<S: RoutingSpace>(
    search: &mut Search<S>,
    space: &S,
    net: &Net<S::Node>,
    options: &SteinerOptions,
) -> Result<SteinerTree<S::Node>, SteinerError> {
    let mut router = Router::new(search, space, net);
    if !net.needs_routing() {
        return Ok(router.finish(Candidate::empty()));
    }
    let mut best = router.best_growth()?;
    // With one other terminal, growth has already found the cheapest path there is.
    if net.terminals.len() >= 3 {
        if let Some(root) = best.route.root()
            && let Some(guided) = router.closure_guided(root, options.closure_neighbours)
            && guided.cost() < best.cost()
        {
            best = guided;
        }
        best = router.refine(best, refine_budget(net, options));
    }
    Ok(router.finish(best))
}

/// Builds a tree by nearest-terminal growth from the driver (Takahashi and Matsuyama 1980).
///
/// Starting from the root, this repeatedly searches from the whole tree to the nearest
/// terminal not yet connected. That takes one search per terminal per access point of the
/// driver, as each access point gets a tree and the cheapest is kept. Branches left with
/// nothing to serve are then removed.
///
/// This is the baseline to compare [`kmb_tree`] with. See the
/// [module documentation](self#guarantees) for what it guarantees.
///
/// # Errors
///
/// Returns [`SteinerError::UnreachableTerminal`] if no access point of the driver reaches
/// all the other terminals.
///
/// # Panics
///
/// Panics if the space breaks the promise of [`RoutingSpace`] by no longer offering a step
/// that it offered during a search.
///
/// # Examples
///
/// ```
/// use guide::cost::Cost;
/// use guide::id::{NetId, NodeId};
/// use guide::net::{Net, Terminal};
/// use guide::search::Search;
/// use guide::space::graph::GraphBuilder;
/// use guide::steiner::{SteinerError, takahashi_matsuyama};
///
/// let mut builder = GraphBuilder::new(3);
/// builder.add_edge(NodeId(0), NodeId(1), Cost::ONE)?;
/// let graph = builder.build();
/// let mut search = Search::new(&graph);
///
/// let terminals = [0, 1].map(|node| Terminal::at(NodeId(node))).to_vec();
/// let net = Net { id: NetId(0), terminals };
/// let tree = takahashi_matsuyama(&mut search, &graph, &net).unwrap();
/// assert_eq!(tree.cost, Cost::ONE);
///
/// let terminals = [0, 1, 2].map(|node| Terminal::at(NodeId(node))).to_vec();
/// let net = Net { id: NetId(0), terminals };
/// assert_eq!(
///     takahashi_matsuyama(&mut search, &graph, &net),
///     Err(SteinerError::UnreachableTerminal { terminal: 2 }),
/// );
/// # Ok::<(), guide::space::graph::GraphError>(())
/// ```
pub fn takahashi_matsuyama<S: RoutingSpace>(
    search: &mut Search<S>,
    space: &S,
    net: &Net<S::Node>,
) -> Result<SteinerTree<S::Node>, SteinerError> {
    let mut router = Router::new(search, space, net);
    if !net.needs_routing() {
        return Ok(router.finish(Candidate::empty()));
    }
    let best = router.best_growth()?;
    Ok(router.finish(best))
}

/// Improves a route for `net`, wherever it came from, as the
/// [module documentation](self#refinement) describes.
///
/// The route is first cleaned up, and is then changed only by replacements that are strictly
/// cheaper, so the result never costs more than the route. Its root stays where it is. A
/// net that does not [need routing](Net::needs_routing) gets an empty route.
///
/// # Errors
///
/// Returns an error if the route is not valid for the net, is not rooted at an access
/// point of the driver, or has an edge that the space does not offer as a step.
///
/// # Panics
///
/// Panics if the space breaks the promise of [`RoutingSpace`] by no longer offering a step
/// that it offered during a search.
///
/// # Examples
///
/// Nearest-first growth takes node 1 first, and pays again for node 2. But node 2 reaches
/// node 1 for free, so taking that edge saves the whole cost of the first.
///
/// ```
/// use guide::cost::Cost;
/// use guide::id::{NetId, NodeId};
/// use guide::net::{Net, Terminal};
/// use guide::search::Search;
/// use guide::space::graph::GraphBuilder;
/// use guide::steiner::{SteinerOptions, refine_tree, takahashi_matsuyama};
///
/// let mut builder = GraphBuilder::new(3);
/// builder.add_edge(NodeId(0), NodeId(1), Cost::from(2))?;
/// builder.add_edge(NodeId(0), NodeId(2), Cost::from(3))?;
/// builder.add_edge(NodeId(2), NodeId(1), Cost::ZERO)?;
/// let graph = builder.build();
///
/// let terminals = [0, 1, 2].map(|node| Terminal::at(NodeId(node))).to_vec();
/// let net = Net { id: NetId(0), terminals };
/// let mut search = Search::new(&graph);
///
/// let grown = takahashi_matsuyama(&mut search, &graph, &net).unwrap();
/// assert_eq!(grown.cost, Cost::from(5));
///
/// let options = SteinerOptions::default();
/// let refined = refine_tree(&mut search, &graph, &net, &grown.route, &options).unwrap();
/// assert_eq!(refined.cost, Cost::from(3));
/// # Ok::<(), guide::space::graph::GraphError>(())
/// ```
pub fn refine_tree<S: RoutingSpace>(
    search: &mut Search<S>,
    space: &S,
    net: &Net<S::Node>,
    route: &Route<S::Node>,
    options: &SteinerOptions,
) -> Result<SteinerTree<S::Node>, RefineError<S::Node>> {
    let mut router = Router::new(search, space, net);
    if !net.needs_routing() {
        return Ok(router.finish(Candidate::empty()));
    }
    route.validate(net).map_err(RefineError::InvalidRoute)?;
    let root = route
        .root()
        .ok_or(RefineError::InvalidRoute(RouteError::MissingTerminal {
            terminal: 0,
        }))?;
    if !net.terminals[0]
        .access_points()
        .iter()
        .any(|point| point.node == root)
    {
        return Err(RefineError::RootNotDriver { root });
    }
    router
        .step_costs(route)
        .map_err(|edge| RefineError::NotAStep {
            from: edge.from,
            to: edge.to,
        })?;
    let start = router.prune(route.clone());
    let start = router.candidate(start);
    let best = router.refine(start, refine_budget(net, options));
    Ok(router.finish(best))
}

fn refine_budget<N>(net: &Net<N>, options: &SteinerOptions) -> u64 {
    let sinks = net.terminals.len().saturating_sub(1) as u64;
    sinks.saturating_mul(options.refine_searches_per_terminal as u64)
}

/// A tree together with its measured cost.
#[derive(Clone, Debug)]
struct Candidate<N> {
    route: Route<N>,
    step_cost: Cost,
    access_cost: Cost,
}

impl<N: Copy> Candidate<N> {
    fn empty() -> Self {
        Candidate {
            route: Route::new(),
            step_cost: Cost::ZERO,
            access_cost: Cost::ZERO,
        }
    }

    fn cost(&self) -> Cost {
        self.step_cost + self.access_cost
    }
}

/// The result of one search for the nearest of several terminals.
#[derive(Clone, Copy)]
struct Reached {
    terminal: usize,
    total: Cost,
}

/// One net's view of the space, with counters and scratch buffers.
struct Router<'a, S: RoutingSpace> {
    search: &'a mut Search<S>,
    space: &'a S,
    net: &'a Net<S::Node>,
    levels: Vec<Cost>,
    targets: Vec<S::Node>,
    /// The path to the terminal most recently returned by `nearest`.
    path: Vec<S::Node>,
    steps: Vec<Step<S::Node>>,
    searches: u64,
    expanded: u64,
}

impl<'a, S: RoutingSpace> Router<'a, S> {
    fn new(search: &'a mut Search<S>, space: &'a S, net: &'a Net<S::Node>) -> Self {
        Router {
            search,
            space,
            net,
            levels: Vec::new(),
            targets: Vec::new(),
            path: Vec::new(),
            steps: Vec::new(),
            searches: 0,
            expanded: 0,
        }
    }

    fn finish(&self, candidate: Candidate<S::Node>) -> SteinerTree<S::Node> {
        SteinerTree {
            cost: candidate.cost(),
            route: candidate.route,
            step_cost: candidate.step_cost,
            access_cost: candidate.access_cost,
            searches: self.searches,
            nodes_expanded: self.expanded,
        }
    }

    /// Finds the terminal in `remaining` that is cheapest to connect from `sources`, with
    /// its access cost included, and leaves the path in `self.path`.
    ///
    /// Search targets carry no cost, so targets are searched one access cost at a time,
    /// lowest first, until no higher cost could win. Nearly every net has one such cost.
    fn nearest(&mut self, sources: &[S::Node], remaining: &[usize]) -> Option<Reached> {
        let terminals = &self.net.terminals;
        self.levels.clear();
        for &terminal in remaining {
            let points = terminals[terminal].access_points();
            self.levels.extend(points.iter().map(|point| point.cost));
        }
        self.levels.sort_unstable();
        self.levels.dedup();

        let mut best: Option<Reached> = None;
        for &level in &self.levels {
            if best.is_some_and(|best| level >= best.total) {
                break;
            }
            self.targets.clear();
            for &terminal in remaining {
                let points = terminals[terminal].access_points();
                let at_level = points.iter().filter(|point| point.cost == level);
                self.targets.extend(at_level.map(|point| point.node));
            }

            self.searches += 1;
            let found = self.search.find_path(
                self.space,
                self.net.id,
                sources.iter().map(|&node| Source::new(node)),
                self.targets.iter().copied(),
            );
            if let Some(found) = found {
                let total = found.cost + level;
                if best.is_none_or(|best| total < best.total) {
                    let end = found.path[found.path.len() - 1];
                    self.path.clear();
                    self.path.extend_from_slice(found.path);
                    let terminal = remaining.iter().copied().find(|&terminal| {
                        terminals[terminal]
                            .access_points()
                            .iter()
                            .any(|point| point.node == end && point.cost == level)
                    });
                    if let Some(terminal) = terminal {
                        best = Some(Reached { terminal, total });
                    }
                }
            }
            self.expanded += self.search.stats().expanded;
        }
        best
    }

    /// Grafts the terminals in `remaining` onto `route`, nearest first, and returns those
    /// that could not be reached.
    fn connect(&mut self, route: &mut Route<S::Node>, mut remaining: Vec<usize>) -> Vec<usize> {
        let mut sources: Vec<S::Node> = route.nodes().collect();
        while !remaining.is_empty() {
            let Some(reached) = self.nearest(&sources, &remaining) else {
                return remaining;
            };
            let before = route.edges().len();
            route
                .add_path(&self.path)
                .expect("a search path starts on the tree and repeats no node");
            sources.extend(route.edges()[before..].iter().map(|edge| edge.to));
            remaining.retain(|&terminal| terminal != reached.terminal);
        }
        remaining
    }

    /// Runs nearest-terminal growth from each access point of the driver and keeps the
    /// cheapest result.
    fn best_growth(&mut self) -> Result<Candidate<S::Node>, SteinerError> {
        let sinks: Vec<usize> = (1..self.net.terminals.len()).collect();
        let mut roots: Vec<S::Node> = Vec::new();
        for point in self.net.terminals[0].access_points() {
            if !roots.contains(&point.node) {
                roots.push(point.node);
            }
        }

        let mut best: Option<Candidate<S::Node>> = None;
        let mut fewest_unreached: Option<Vec<usize>> = None;
        for root in roots {
            let mut route = Route::from_root(root);
            let unreached = self.connect(&mut route, sinks.clone());
            if unreached.is_empty() {
                let route = self.prune(route);
                let candidate = self.candidate(route);
                if best
                    .as_ref()
                    .is_none_or(|best| candidate.cost() < best.cost())
                {
                    best = Some(candidate);
                }
            } else if fewest_unreached
                .as_ref()
                .is_none_or(|fewest| unreached.len() < fewest.len())
            {
                fewest_unreached = Some(unreached);
            }
        }
        best.ok_or_else(|| SteinerError::UnreachableTerminal {
            terminal: fewest_unreached
                .and_then(|unreached| unreached.iter().copied().min())
                .unwrap_or(0),
        })
    }

    /// Builds a tree from `root` in the order that a minimum arborescence of the closure
    /// gives, or returns `None` if the closure is unusable.
    fn closure_guided(&mut self, root: S::Node, neighbours: usize) -> Option<Candidate<S::Node>> {
        let count = self.net.terminals.len();
        let (edges, starts) = self.closure(root, neighbours);
        let chosen = min_arborescence(count, 0, &edges)?;
        let (order, onward) = connection_order(count, &edges, &chosen);

        let free = self.realise(root, &order, &vec![None; count])?;
        let has_choices = self.net.terminals[1..]
            .iter()
            .any(|terminal| terminal.access_points().len() > 1);
        if !has_choices {
            return Some(free);
        }

        // The closure counts on leaving a terminal from the access point where an edge
        // out of it starts, but free growth may well arrive at a different one, stranding
        // that edge. So try again, arriving where the cheapest edge out wants to leave.
        let wanted: Vec<Option<S::Node>> = onward
            .iter()
            .map(|edge| edge.map(|edge| starts[edge]))
            .collect();
        match self.realise(root, &order, &wanted) {
            Some(hinted) if hinted.cost() < free.cost() => Some(hinted),
            _ => Some(free),
        }
    }

    /// Grows a tree from `root`, connecting terminals in `order`. A terminal arrives at the
    /// access point in `wanted` if it has an entry there, and at its nearest otherwise.
    fn realise(
        &mut self,
        root: S::Node,
        order: &[usize],
        wanted: &[Option<S::Node>],
    ) -> Option<Candidate<S::Node>> {
        let mut route = Route::from_root(root);
        for &terminal in order {
            let arrived = wanted[terminal].is_some_and(|node| self.connect_to(&mut route, node));
            if !arrived && !self.connect(&mut route, vec![terminal]).is_empty() {
                return None;
            }
        }
        let route = self.prune(route);
        Some(self.candidate(route))
    }

    /// Grafts the cheapest path to `node` onto `route`, and returns whether there is one.
    fn connect_to(&mut self, route: &mut Route<S::Node>, node: S::Node) -> bool {
        self.searches += 1;
        let found = self.search.find_path(
            self.space,
            self.net.id,
            route.nodes().map(Source::new),
            [node],
        );
        let connected = found.is_some_and(|found| {
            route
                .add_path(found.path)
                .expect("a search path starts on the tree and repeats no node");
            true
        });
        self.expanded += self.search.stats().expanded;
        connected
    }

    /// Finds the closure's edges, in ascending order of `(from, to)`, and the node each
    /// one's path starts from.
    ///
    /// The driver's row starts from `root` only, because that is the node the tree grows
    /// from. No edge enters the driver, because no arborescence uses one.
    fn closure(&mut self, root: S::Node, neighbours: usize) -> (Vec<ClosureEdge>, Vec<S::Node>) {
        let count = self.net.terminals.len();
        let mut edges = Vec::new();
        for from in 0..count {
            let sources: Vec<S::Node> = if from == 0 {
                vec![root]
            } else {
                let points = self.net.terminals[from].access_points();
                points.iter().map(|point| point.node).collect()
            };
            let limit = if from == 0 { usize::MAX } else { neighbours };
            let mut remaining: Vec<usize> = (1..count).filter(|&to| to != from).collect();
            let first = edges.len();
            for _ in 0..limit {
                if remaining.is_empty() {
                    break;
                }
                let Some(reached) = self.nearest(&sources, &remaining) else {
                    break;
                };
                edges.push((
                    ClosureEdge {
                        from,
                        to: reached.terminal,
                        weight: reached.total.get(),
                    },
                    self.path[0],
                ));
                remaining.retain(|&to| to != reached.terminal);
            }
            edges[first..].sort_unstable_by_key(|(edge, _)| edge.to);
        }
        edges.into_iter().unzip()
    }

    /// Returns the cost of each edge of `route`, or the first edge that is not a step.
    fn step_costs(&mut self, route: &Route<S::Node>) -> Result<Vec<Cost>, Edge<S::Node>> {
        let mut costs = Vec::with_capacity(route.edges().len());
        for &edge in route.edges() {
            self.steps.clear();
            self.space.expand(self.net.id, edge.from, &mut self.steps);
            let cost = self
                .steps
                .iter()
                .filter(|step| step.to == edge.to)
                .map(|step| step.cost)
                .min();
            costs.push(cost.ok_or(edge)?);
        }
        Ok(costs)
    }

    /// Measures a tree whose edges came from searches over this space.
    fn candidate(&mut self, route: Route<S::Node>) -> Candidate<S::Node> {
        let step_cost = self
            .step_costs(&route)
            .expect("a space must offer the same steps every time it is asked")
            .into_iter()
            .sum();
        let access_cost = self.access_cost(&route);
        Candidate {
            route,
            step_cost,
            access_cost,
        }
    }

    /// Returns the access cost of `route`: that of the driver at the root, plus that of
    /// each other terminal at its cheapest access point on the route.
    fn access_cost(&self, route: &Route<S::Node>) -> Cost {
        let nodes: HashSet<S::Node> = route.nodes().collect();
        let root = route.root();
        let cheapest = |terminal: usize| {
            self.net.terminals[terminal]
                .access_points()
                .iter()
                .filter(|point| {
                    if terminal == 0 {
                        Some(point.node) == root
                    } else {
                        nodes.contains(&point.node)
                    }
                })
                .map(|point| point.cost)
                .min()
                .unwrap_or(Cost::ZERO)
        };
        (0..self.net.terminals.len()).map(cheapest).sum()
    }

    /// Works out how the nodes of `route` relate to each other and to the terminals.
    fn shape(&mut self, route: &Route<S::Node>) -> Shape<S::Node> {
        let nodes: Vec<S::Node> = route.nodes().collect();
        let index: HashMap<S::Node, usize> = nodes
            .iter()
            .enumerate()
            .map(|(position, &node)| (node, position))
            .collect();
        let mut parent = vec![0; nodes.len()];
        let mut children = vec![Vec::new(); nodes.len()];
        for (edge_index, edge) in route.edges().iter().enumerate() {
            parent[edge_index + 1] = index[&edge.from];
            children[index[&edge.from]].push(edge_index + 1);
        }
        let mut step = vec![Cost::ZERO];
        step.extend(
            self.step_costs(route)
                .expect("a space must offer the same steps every time it is asked"),
        );

        let mut holders = vec![Vec::new(); self.net.terminals.len()];
        let mut held = vec![Vec::new(); nodes.len()];
        for (terminal, candidates) in self.net.terminals.iter().enumerate().skip(1) {
            for point in candidates.access_points() {
                if let Some(&position) = index.get(&point.node) {
                    holders[terminal].push((position, point.cost));
                    held[position].push(terminal);
                }
            }
        }
        Shape {
            nodes,
            parent,
            children,
            step,
            holders,
            held,
        }
    }

    /// Removes branches that serve no terminal, or that cost more than they save.
    ///
    /// A branch can serve a terminal and still go, if another access point of the same
    /// terminal is on the tree, so this works over the terminals rather than just looking
    /// for leaves with nothing on them.
    fn prune(&mut self, route: Route<S::Node>) -> Route<S::Node> {
        if route.edges().is_empty() {
            return route;
        }
        let shape = self.shape(&route);
        let count = shape.nodes.len();
        let mut alive = vec![true; count];
        let mut child_count: Vec<usize> = shape.children.iter().map(Vec::len).collect();
        let mut touched: Vec<usize> = Vec::new();

        let mut changed = true;
        while changed {
            changed = false;
            for leaf in (1..count).rev() {
                if !alive[leaf] || child_count[leaf] != 0 {
                    continue;
                }

                // Walks up from the leaf, removing nodes tentatively, for as long as the
                // walk stays on a chain with nothing branching off it.
                let mut chain = Vec::new();
                let mut saved = Cost::ZERO;
                let mut extra_access = 0.0;
                let mut best_gain = None;
                let mut best_len = 0;
                touched.clear();
                let mut node = leaf;
                loop {
                    alive[node] = false;
                    chain.push(node);
                    saved = saved + shape.step[node];

                    if !shape.held[node].is_empty() {
                        for &terminal in &shape.held[node] {
                            if !touched.contains(&terminal) {
                                touched.push(terminal);
                            }
                        }
                        let change = shape.access_change(&touched, &chain, &alive);
                        let Some(change) = change else {
                            // A terminal would be lost, here and further up.
                            break;
                        };
                        extra_access = change;
                    }
                    let gain = saved.get() - extra_access;
                    if best_gain.is_none_or(|best| gain >= best) {
                        best_gain = Some(gain);
                        best_len = chain.len();
                    }

                    let up = shape.parent[node];
                    if up == 0 || child_count[up] != 1 {
                        break;
                    }
                    node = up;
                }

                let removed = if best_gain.is_some_and(|gain| gain >= 0.0) {
                    best_len
                } else {
                    0
                };
                for &node in &chain[removed..] {
                    alive[node] = true;
                }
                if removed > 0 {
                    child_count[shape.parent[chain[removed - 1]]] -= 1;
                    changed = true;
                }
            }
        }

        let edges = route
            .edges()
            .iter()
            .enumerate()
            .filter(|&(edge_index, _)| alive[edge_index + 1])
            .map(|(_, &edge)| edge)
            .collect();
        Route::from_edges(shape.nodes[0], edges)
    }

    /// Repeatedly removes a key path with everything below it and reconnects what that
    /// lost, keeping the result whenever it is cheaper, until a whole pass finds nothing
    /// or `budget` searches have run.
    fn refine(&mut self, mut best: Candidate<S::Node>, budget: u64) -> Candidate<S::Node> {
        let start = self.searches;
        loop {
            let mut improved = false;
            let mut keys = self.key_nodes(&best.route);
            let mut position = 0;
            while position < keys.len() {
                if self.searches - start >= budget {
                    return best;
                }
                if let Some(better) = self.exchange(&best, keys[position]) {
                    best = better;
                    improved = true;
                    keys = self.key_nodes(&best.route);
                }
                position += 1;
            }
            if !improved {
                return best;
            }
        }
    }

    /// Returns the nodes that key paths end at, in route order: those holding an access
    /// point of a terminal other than the driver, and those where the tree branches.
    fn key_nodes(&mut self, route: &Route<S::Node>) -> Vec<S::Node> {
        let shape = self.shape(route);
        (1..shape.nodes.len())
            .rev()
            .filter(|&node| !shape.held[node].is_empty() || shape.children[node].len() > 1)
            .map(|node| shape.nodes[node])
            .collect()
    }

    /// Tries replacing the key path ending at `key`, and returns the result if cheaper.
    fn exchange(
        &mut self,
        current: &Candidate<S::Node>,
        key: S::Node,
    ) -> Option<Candidate<S::Node>> {
        let shape = self.shape(&current.route);
        let key = shape.nodes.iter().position(|&node| node == key)?;

        // Everything below the key node goes, and so does the path above it, up to the
        // nearest node that is still needed for something else.
        let mut alive = vec![true; shape.nodes.len()];
        let mut stack = vec![key];
        while let Some(node) = stack.pop() {
            alive[node] = false;
            stack.extend_from_slice(&shape.children[node]);
        }
        let mut node = shape.parent[key];
        while node != 0 && shape.children[node].len() == 1 && shape.held[node].is_empty() {
            alive[node] = false;
            node = shape.parent[node];
        }

        // Terminals still on the tree, but only at a dearer access point, also get another
        // chance, as they can come out better by extending the tree.
        let mut lost = Vec::new();
        for terminal in 1..self.net.terminals.len() {
            let holders = &shape.holders[terminal];
            let before = holders.iter().map(|&(_, cost)| cost).min();
            let after = holders
                .iter()
                .filter(|&&(node, _)| alive[node])
                .map(|&(_, cost)| cost)
                .min();
            if before != after {
                lost.push(terminal);
            }
        }

        let edges = current
            .route
            .edges()
            .iter()
            .enumerate()
            .filter(|&(edge_index, _)| alive[edge_index + 1])
            .map(|(_, &edge)| edge)
            .collect();
        let mut route = Route::from_edges(shape.nodes[0], edges);
        if !self.connect(&mut route, lost).is_empty() {
            return None;
        }
        let route = self.prune(route);
        let candidate = self.candidate(route);
        (candidate.cost() < current.cost()).then_some(candidate)
    }
}

/// The structure of a tree, by position in [`Route::nodes`].
struct Shape<N> {
    nodes: Vec<N>,
    parent: Vec<usize>,
    children: Vec<Vec<usize>>,
    /// The cost of the step into each node.
    step: Vec<Cost>,
    /// For each terminal except the driver, its access points on the tree.
    holders: Vec<Vec<(usize, Cost)>>,
    /// For each node, the terminals other than the driver with an access point there.
    held: Vec<Vec<usize>>,
}

impl<N> Shape<N> {
    /// Returns how much more `terminals` would pay in access costs with the nodes in
    /// `removed` gone, or `None` if one of them would lose its connection. `alive` must
    /// already show those nodes as gone.
    fn access_change(&self, terminals: &[usize], removed: &[usize], alive: &[bool]) -> Option<f64> {
        let mut change = 0.0;
        for &terminal in terminals {
            let holders = &self.holders[terminal];
            let with = holders
                .iter()
                .filter(|&&(node, _)| alive[node] || removed.contains(&node))
                .map(|&(_, cost)| cost)
                .min()?;
            let without = holders
                .iter()
                .filter(|&&(node, _)| alive[node])
                .map(|&(_, cost)| cost)
                .min()?;
            change += without.get() - with.get();
        }
        Some(change)
    }
}

/// An edge of the closure over the terminals.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ClosureEdge {
    from: usize,
    to: usize,
    weight: f64,
}

/// Lists the terminals other than the driver in the order a tree should connect them:
/// depth first over the arborescence, with the cheapest edge first at each terminal.
/// Also returns, for each terminal, that cheapest edge out of it.
fn connection_order(
    count: usize,
    edges: &[ClosureEdge],
    chosen: &[usize],
) -> (Vec<usize>, Vec<Option<usize>>) {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
    for &edge in chosen {
        children[edges[edge].from].push(edge);
    }
    let mut order = Vec::with_capacity(count.saturating_sub(1));
    let mut onward = vec![None; count];
    let mut stack = vec![0];
    while let Some(terminal) = stack.pop() {
        if terminal != 0 {
            order.push(terminal);
        }
        let list = &mut children[terminal];
        list.sort_by(|&a, &b| {
            let (a, b) = (&edges[a], &edges[b]);
            a.weight.total_cmp(&b.weight).then(a.to.cmp(&b.to))
        });
        onward[terminal] = list.first().copied();
        stack.extend(list.iter().rev().map(|&edge| edges[edge].to));
    }
    (order, onward)
}

/// Finds a minimum arborescence by the Chu-Liu/Edmonds algorithm, as indices into `edges`.
///
/// Returns `None` if `root` does not reach every node. Among edges of equal reduced weight
/// the earliest in `edges` wins. This runs in `O(nodes * edges)`, and uses a loop rather
/// than recursion so that its depth cannot exhaust the stack.
fn min_arborescence(nodes: usize, root: usize, edges: &[ClosureEdge]) -> Option<Vec<usize>> {
    const NONE: usize = usize::MAX;

    /// One contraction: what the graph looked like, and where the next one's edges came from.
    struct Level {
        nodes: usize,
        root: usize,
        tos: Vec<usize>,
        best: Vec<usize>,
        origins: Vec<usize>,
    }

    let mut levels: Vec<Level> = Vec::new();
    let mut current = edges.to_vec();
    let (mut nodes, mut root) = (nodes, root);
    let mut chosen = loop {
        // The cheapest edge into each node. Taken together these are the answer, unless
        // they form cycles.
        let mut best = vec![NONE; nodes];
        for (index, edge) in current.iter().enumerate() {
            if edge.to != root
                && edge.from != edge.to
                && (best[edge.to] == NONE || edge.weight < current[best[edge.to]].weight)
            {
                best[edge.to] = index;
            }
        }
        if (0..nodes).any(|node| node != root && best[node] == NONE) {
            return None;
        }

        // Each node has one chosen parent, so walking up ends at the root, at a node seen
        // by an earlier walk, or at a node seen by this walk, which closes a cycle.
        let mut component = vec![NONE; nodes];
        let mut on_cycle = vec![false; nodes];
        let mut seen = vec![NONE; nodes];
        let mut components = 0;
        for start in 0..nodes {
            let mut node = start;
            while node != root && seen[node] == NONE {
                seen[node] = start;
                node = current[best[node]].from;
            }
            if node != root && seen[node] == start {
                let mut member = node;
                loop {
                    on_cycle[member] = true;
                    component[member] = components;
                    member = current[best[member]].from;
                    if member == node {
                        break;
                    }
                }
                components += 1;
            }
        }
        if components == 0 {
            break (0..nodes)
                .filter(|&node| node != root)
                .map(|node| best[node])
                .collect::<Vec<usize>>();
        }
        for slot in &mut component {
            if *slot == NONE {
                *slot = components;
                components += 1;
            }
        }

        // Contract each cycle. Entering one at a node saves the edge that node has now,
        // which is what reducing the weight accounts for.
        let mut next = Vec::with_capacity(current.len());
        let mut origins = Vec::with_capacity(current.len());
        for (index, edge) in current.iter().enumerate() {
            let (from, to) = (component[edge.from], component[edge.to]);
            if from != to {
                let saved = if on_cycle[edge.to] {
                    current[best[edge.to]].weight
                } else {
                    0.0
                };
                next.push(ClosureEdge {
                    from,
                    to,
                    weight: edge.weight - saved,
                });
                origins.push(index);
            }
        }
        levels.push(Level {
            nodes,
            root,
            tos: current.iter().map(|edge| edge.to).collect(),
            best,
            origins,
        });
        root = component[root];
        nodes = components;
        current = next;
    };

    // Undo the contractions, last first. An edge chosen at the level below enters one node
    // here. All other nodes keep their cheapest edge, which is only the case on a cycle.
    while let Some(level) = levels.pop() {
        let mut incoming = vec![NONE; level.nodes];
        for &edge in &chosen {
            let origin = level.origins[edge];
            incoming[level.tos[origin]] = origin;
        }
        chosen = (0..level.nodes)
            .filter(|&node| node != level.root)
            .map(|node| {
                if incoming[node] == NONE {
                    level.best[node]
                } else {
                    incoming[node]
                }
            })
            .collect();
    }
    Some(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{NetId, NodeId};
    use crate::net::{AccessPoint, Terminal};
    use crate::rng::Rng;
    use crate::space::InternedSlots;
    use crate::space::graph::{Graph, GraphBuilder};
    use crate::space::grid::{Connectivity, Grid};
    use proptest::collection::vec;
    use proptest::prelude::*;

    const NET: NetId = NetId(0);

    /// The cheapest tree for `net`, by the Dreyfus-Wagner dynamic programme over subsets
    /// of terminals, extended to groups by starting each subset of one terminal from the
    /// cheapest of its access points. Step costs are whatever `expand` says.
    fn exact_cost<S: RoutingSpace<Node = NodeId>>(
        space: &S,
        nodes: usize,
        net: &Net<NodeId>,
    ) -> Option<f64> {
        let sinks = net.terminals.len().checked_sub(1)?;
        let mut distance = vec![vec![f64::INFINITY; nodes]; nodes];
        let mut out = Vec::new();
        for (from, row) in distance.iter_mut().enumerate() {
            row[from] = 0.0;
            out.clear();
            space.expand(NET, NodeId(from as u32), &mut out);
            for step in &out {
                let slot = &mut row[step.to.index()];
                *slot = slot.min(step.cost.get());
            }
        }
        for via in 0..nodes {
            for from in 0..nodes {
                for to in 0..nodes {
                    let through = distance[from][via] + distance[via][to];
                    if through < distance[from][to] {
                        distance[from][to] = through;
                    }
                }
            }
        }

        let full = (1usize << sinks) - 1;
        let mut table = vec![vec![f64::INFINITY; nodes]; full + 1];
        table[0] = vec![0.0; nodes];
        for mask in 1..=full {
            let mut row = vec![f64::INFINITY; nodes];
            if mask.is_power_of_two() {
                let terminal = &net.terminals[mask.trailing_zeros() as usize + 1];
                for point in terminal.access_points() {
                    for (from, value) in row.iter_mut().enumerate() {
                        let total = distance[from][point.node.index()] + point.cost.get();
                        *value = value.min(total);
                    }
                }
            } else {
                let mut part = (mask - 1) & mask;
                while part > 0 {
                    for (node, value) in row.iter_mut().enumerate() {
                        *value = value.min(table[part][node] + table[mask ^ part][node]);
                    }
                    part = (part - 1) & mask;
                }
                let merged = row.clone();
                for (from, value) in row.iter_mut().enumerate() {
                    for (via, &there) in merged.iter().enumerate() {
                        *value = value.min(distance[from][via] + there);
                    }
                }
            }
            table[mask] = row;
        }
        net.terminals[0]
            .access_points()
            .iter()
            .map(|point| point.cost.get() + table[full][point.node.index()])
            .filter(|cost| cost.is_finite())
            .min_by(f64::total_cmp)
    }

    /// The textbook KMB expansion, for comparison: the union of the closure's paths, reduced
    /// to a tree by shortest paths from the root, or by a minimum arborescence.
    fn classic_kmb<S: RoutingSpace<Node = NodeId>>(
        search: &mut Search<S>,
        space: &S,
        net: &Net<NodeId>,
        use_arborescence: bool,
    ) -> Option<f64> {
        let mut router = Router::new(search, space, net);
        let root = net.terminals[0].access_points()[0].node;
        let (edges, _) = router.closure(root, usize::MAX);
        let chosen = min_arborescence(net.terminals.len(), 0, &edges)?;
        let mut union: Vec<(NodeId, NodeId)> = Vec::new();
        for &edge in &chosen {
            let edge = edges[edge];
            let from = net.terminals[edge.from].access_points()[0].node;
            let to = net.terminals[edge.to].access_points()[0].node;
            let found = router
                .search
                .find_path(space, NET, [Source::new(from)], [to])?;
            for pair in found.path.windows(2) {
                if !union.contains(&(pair[0], pair[1])) {
                    union.push((pair[0], pair[1]));
                }
            }
        }
        let mut nodes = vec![root];
        for &(from, to) in &union {
            for node in [from, to] {
                if !nodes.contains(&node) {
                    nodes.push(node);
                }
            }
        }
        let position = |node| nodes.iter().position(|&other| other == node).unwrap();
        let mut weighted = Vec::new();
        for &(from, to) in &union {
            let route = Route::from_edges(from, vec![Edge { from, to }]);
            let cost = router.step_costs(&route).ok()?[0].get();
            weighted.push(ClosureEdge {
                from: position(from),
                to: position(to),
                weight: cost,
            });
        }
        let tree: Vec<Edge<NodeId>> = if use_arborescence {
            let chosen = min_arborescence(nodes.len(), 0, &weighted)?;
            chosen
                .iter()
                .map(|&edge| Edge {
                    from: nodes[weighted[edge].from],
                    to: nodes[weighted[edge].to],
                })
                .collect()
        } else {
            let mut distance = vec![f64::INFINITY; nodes.len()];
            let mut parent = vec![0; nodes.len()];
            distance[0] = 0.0;
            for _ in 0..nodes.len() {
                for edge in &weighted {
                    if distance[edge.from] + edge.weight < distance[edge.to] {
                        distance[edge.to] = distance[edge.from] + edge.weight;
                        parent[edge.to] = edge.from;
                    }
                }
            }
            (1..nodes.len())
                .map(|node| Edge {
                    from: nodes[parent[node]],
                    to: nodes[node],
                })
                .collect()
        };
        let route = router.prune(Route::from_edges(root, tree));
        Some(router.candidate(route).cost().get())
    }

    /// Terminals drawn from `nodes`, each with up to `points` access points.
    fn random_net(
        rng: &mut Rng,
        nodes: &[u32],
        terminals: usize,
        points: u64,
        priced: bool,
    ) -> Net<NodeId> {
        let terminals = (0..terminals)
            .map(|_| {
                let count = 1 + rng.below(points);
                let list = (0..count)
                    .map(|_| AccessPoint {
                        node: NodeId(nodes[rng.below(nodes.len() as u64) as usize]),
                        cost: if priced {
                            Cost::from(rng.below(4) as u32)
                        } else {
                            Cost::ZERO
                        },
                    })
                    .collect();
                Terminal::new(list).unwrap()
            })
            .collect();
        Net { id: NET, terminals }
    }

    fn random_grid(rng: &mut Rng, layers: u32, blocked: u64) -> (Grid, Vec<u32>) {
        let width = 4 + rng.below(3) as u32;
        let height = 4 + rng.below(3) as u32;
        let mut grid = Grid::builder(width, height, layers)
            .via_cost(3.0)
            .build()
            .unwrap();
        for node in 0..grid.cell_count() {
            if rng.below(100) < blocked {
                let (x, y, layer) = grid.position(NodeId(node)).unwrap();
                grid.set_blocked(x, y, layer, true).unwrap();
            }
        }
        let free = (0..grid.cell_count())
            .filter(|&node| !grid.is_blocked(NodeId(node)))
            .collect();
        (grid, free)
    }

    fn random_graph(rng: &mut Rng, symmetric: bool) -> (Graph, Vec<u32>) {
        let nodes = 8 + rng.below(13) as u32;
        let edges = u64::from(nodes) * (2 + rng.below(3));
        let mut builder = GraphBuilder::new(nodes);
        for _ in 0..edges {
            let from = NodeId(rng.below(u64::from(nodes)) as u32);
            let to = NodeId(rng.below(u64::from(nodes)) as u32);
            let cost = Cost::from(1 + rng.below(20) as u32);
            builder.add_edge(from, to, cost).unwrap();
            if symmetric {
                builder.add_edge(to, from, cost).unwrap();
            }
        }
        (builder.build(), (0..nodes).collect())
    }

    /// Layers of nodes fanning out from the first, like wires in a routing fabric.
    fn random_fabric(rng: &mut Rng) -> (Graph, Vec<u32>) {
        let (depth, width) = (3 + rng.below(2) as u32, 3 + rng.below(3) as u32);
        let nodes = 1 + depth * width;
        let node = |layer: u32, index: u32| NodeId(1 + layer * width + index);
        let mut builder = GraphBuilder::new(nodes);
        for index in 0..width {
            let cost = Cost::from(1 + rng.below(9) as u32);
            builder.add_edge(NodeId(0), node(0, index), cost).unwrap();
        }
        for layer in 0..depth - 1 {
            for index in 0..width {
                for _ in 0..2 + rng.below(2) {
                    let to = node(layer + 1, rng.below(u64::from(width)) as u32);
                    let cost = Cost::from(1 + rng.below(9) as u32);
                    builder.add_edge(node(layer, index), to, cost).unwrap();
                }
            }
        }
        let list = (1..nodes).collect();
        (builder.build(), list)
    }

    #[derive(Default)]
    struct Series {
        ratios: Vec<f64>,
        against_growth: Vec<f64>,
        searches: u64,
    }

    impl Series {
        fn add(&mut self, cost: f64, optimum: f64, growth: f64, searches: u64) {
            let ratio = |a: f64, b: f64| if b > 0.0 { a / b } else { 1.0 };
            self.ratios.push(ratio(cost, optimum));
            self.against_growth.push(ratio(cost, growth));
            self.searches += searches;
        }

        fn report(&mut self, name: &str) {
            if self.ratios.is_empty() {
                return;
            }
            self.ratios.sort_by(f64::total_cmp);
            let count = self.ratios.len();
            let mean = self.ratios.iter().sum::<f64>() / count as f64;
            let optimal = self.ratios.iter().filter(|&&r| r <= 1.0 + 1e-9).count();
            let better = self
                .against_growth
                .iter()
                .filter(|&&r| r < 1.0 - 1e-9)
                .count();
            let worse = self
                .against_growth
                .iter()
                .filter(|&&r| r > 1.0 + 1e-9)
                .count();
            let versus = self.against_growth.iter().sum::<f64>() / count as f64;
            println!(
                "  {name:<22} mean {mean:.4}  p99 {:.3}  max {:.3}  optimal {:5.1}%  | vs growth {versus:.4}  better {:5.1}%  worse {:5.1}%  | searches {:.1}",
                self.ratios[count * 99 / 100],
                self.ratios[count - 1],
                100.0 * optimal as f64 / count as f64,
                100.0 * better as f64 / count as f64,
                100.0 * worse as f64 / count as f64,
                self.searches as f64 / count as f64,
            );
        }
    }

    /// Compares every variant with the optimum over `sample` random nets. Classic KMB needs
    /// one access point per terminal, so it only runs if `classic` says the family has them.
    fn measure<S: RoutingSpace<Node = NodeId>>(
        family: &str,
        sample: usize,
        classic: bool,
        rng: &mut Rng,
        mut make: impl FnMut(&mut Rng) -> (S, usize, Net<NodeId>),
    ) {
        let without_refining = SteinerOptions {
            refine_searches_per_terminal: 0,
            ..SteinerOptions::default()
        };
        let full_closure = SteinerOptions {
            closure_neighbours: usize::MAX,
            ..SteinerOptions::default()
        };
        let mut growth = Series::default();
        let mut growth_refined = Series::default();
        let mut guided = Series::default();
        let mut unrefined = Series::default();
        let mut defaults = Series::default();
        let mut full = Series::default();
        let mut spt = Series::default();
        let mut msa = Series::default();
        let mut classic_differs = 0;

        let mut done = 0;
        while done < sample {
            let (space, nodes, net) = make(rng);
            let mut search = Search::new(&space);
            let Some(optimum) = exact_cost(&space, nodes, &net) else {
                continue;
            };
            done += 1;

            let base = takahashi_matsuyama(&mut search, &space, &net).unwrap();
            let growth_cost = base.cost.get();
            growth.add(growth_cost, optimum, growth_cost, base.searches);

            let options = SteinerOptions::default();
            let tree = refine_tree(&mut search, &space, &net, &base.route, &options).unwrap();
            growth_refined.add(
                tree.cost.get(),
                optimum,
                growth_cost,
                base.searches + tree.searches,
            );

            let mut router = Router::new(&mut search, &space, &net);
            let root = base.route.root().unwrap();
            let only = router.closure_guided(root, usize::MAX).unwrap();
            let searches = base.searches + router.searches;
            guided.add(only.cost().get(), optimum, growth_cost, searches);

            let tree = kmb_tree(&mut search, &space, &net, &without_refining).unwrap();
            unrefined.add(tree.cost.get(), optimum, growth_cost, tree.searches);
            let tree = kmb_tree(&mut search, &space, &net, &options).unwrap();
            defaults.add(tree.cost.get(), optimum, growth_cost, tree.searches);
            let tree = kmb_tree(&mut search, &space, &net, &full_closure).unwrap();
            full.add(tree.cost.get(), optimum, growth_cost, tree.searches);

            if classic {
                let by_paths = classic_kmb(&mut search, &space, &net, false).unwrap();
                let by_arborescence = classic_kmb(&mut search, &space, &net, true).unwrap();
                spt.add(by_paths, optimum, growth_cost, 0);
                msa.add(by_arborescence, optimum, growth_cost, 0);
                if by_paths != by_arborescence {
                    classic_differs += 1;
                }
            }
        }
        println!("{family} ({sample} nets)");
        growth.report("growth");
        growth_refined.report("growth, refined");
        guided.report("closure-guided alone");
        unrefined.report("kmb_tree, no refining");
        defaults.report("kmb_tree, defaults");
        full.report("kmb_tree, full closure");
        spt.report("classic KMB, SPT");
        msa.report("classic KMB, MSA");
        if classic {
            println!("  classic expansions differ on {classic_differs} nets");
        }
    }

    /// Compares settings with growth on larger nets, where no optimum is available.
    fn measure_against_growth<S: RoutingSpace<Node = NodeId>>(
        family: &str,
        sample: usize,
        rng: &mut Rng,
        mut make: impl FnMut(&mut Rng) -> (S, Net<NodeId>),
    ) {
        use std::time::Instant;
        let settings: &[(&str, usize, usize)] = &[
            ("closure only", 8, 0),
            ("r=4", 8, 4),
            ("defaults", 8, 8),
            ("r=16", 8, 16),
            ("full closure", usize::MAX, 8),
        ];
        let mut ratios = vec![0.0; settings.len()];
        let mut arrivals = vec![0.0; settings.len()];
        let mut searches = vec![0u64; settings.len()];
        let mut times = vec![0.0; settings.len()];
        let (mut base_searches, mut base_time) = (0, 0.0);
        let (mut refined_ratio, mut refined_arrival, mut refined_searches, mut refined_time) =
            (0.0, 0.0, 0, 0.0);
        let mut done = 0;
        while done < sample {
            let (space, net) = make(rng);
            let mut search = Search::new(&space);
            let start = Instant::now();
            let Ok(base) = takahashi_matsuyama(&mut search, &space, &net) else {
                continue;
            };
            base_time += start.elapsed().as_secs_f64();
            base_searches += base.searches;
            let base_arrival = latest_arrival(&space, &net, &base.route);
            done += 1;

            let options = SteinerOptions::default();
            let start = Instant::now();
            let tree = refine_tree(&mut search, &space, &net, &base.route, &options).unwrap();
            refined_time += start.elapsed().as_secs_f64();
            refined_searches += tree.searches;
            refined_ratio += tree.cost.get() / base.cost.get();
            refined_arrival += latest_arrival(&space, &net, &tree.route) / base_arrival;

            for (index, &(_, closure_neighbours, refine_searches_per_terminal)) in
                settings.iter().enumerate()
            {
                let options = SteinerOptions {
                    closure_neighbours,
                    refine_searches_per_terminal,
                };
                let start = Instant::now();
                let tree = kmb_tree(&mut search, &space, &net, &options).unwrap();
                times[index] += start.elapsed().as_secs_f64();
                searches[index] += tree.searches;
                ratios[index] += tree.cost.get() / base.cost.get();
                arrivals[index] += latest_arrival(&space, &net, &tree.route) / base_arrival;
            }
        }
        let scale = sample as f64;
        println!(
            "{family} ({sample} nets): growth {:.0} searches, {:.2} ms",
            base_searches as f64 / scale,
            1e3 * base_time / scale
        );
        println!(
            "  {:<14} cost {:.4}  latest arrival {:.3}  searches {:7.0}  {:8.2} ms",
            "growth+refine",
            refined_ratio / scale,
            refined_arrival / scale,
            (base_searches + refined_searches) as f64 / scale,
            1e3 * (base_time + refined_time) / scale
        );
        for (index, setting) in settings.iter().enumerate() {
            println!(
                "  {:<14} cost {:.4}  latest arrival {:.3}  searches {:7.0}  {:8.2} ms",
                setting.0,
                ratios[index] / scale,
                arrivals[index] / scale,
                searches[index] as f64 / scale,
                1e3 * times[index] / scale
            );
        }
    }

    /// The cost from the root to the terminal farthest along the tree, a stand-in for delay.
    fn latest_arrival<S: RoutingSpace<Node = NodeId>>(
        space: &S,
        net: &Net<NodeId>,
        route: &Route<NodeId>,
    ) -> f64 {
        let mut search = Search::new(space);
        let mut router = Router::new(&mut search, space, net);
        let shape = router.shape(route);
        let arrival = |mut node: usize| {
            let mut total = 0.0;
            while node != 0 {
                total += shape.step[node].get();
                node = shape.parent[node];
            }
            total
        };
        let earliest = shape.holders.iter().skip(1).map(|holders| {
            holders
                .iter()
                .map(|&(node, _)| arrival(node))
                .min_by(f64::total_cmp)
                .unwrap_or(0.0)
        });
        earliest.max_by(f64::total_cmp).unwrap_or(0.0).max(1e-9)
    }

    #[test]
    #[ignore = "a long measurement: run with --release --ignored --nocapture"]
    fn measure_large_nets_against_growth() {
        let mut rng = Rng::new(2026);
        for &(size, terminals, sample) in &[
            (32u32, 5usize, 200usize),
            (64, 10, 100),
            (64, 25, 50),
            (128, 50, 12),
            (128, 100, 5),
        ] {
            measure_against_growth(
                &format!("{size}x{size} grid, {terminals} terminals"),
                sample,
                &mut rng,
                |rng| {
                    let mut grid = Grid::new(size, size, 1).unwrap();
                    for node in 0..grid.cell_count() {
                        if rng.below(100) < 10 {
                            let (x, y, layer) = grid.position(NodeId(node)).unwrap();
                            grid.set_blocked(x, y, layer, true).unwrap();
                        }
                    }
                    let free: Vec<u32> = (0..grid.cell_count())
                        .filter(|&node| !grid.is_blocked(NodeId(node)))
                        .collect();
                    let net = random_net(rng, &free, terminals, 1, false);
                    (grid, net)
                },
            );
        }
        for &(nodes, terminals, sample) in &[
            (500u32, 5usize, 200usize),
            (2000, 10, 100),
            (2000, 25, 50),
            (10000, 50, 12),
            (10000, 100, 5),
        ] {
            measure_against_growth(
                &format!("directed graph of {nodes}, {terminals} terminals"),
                sample,
                &mut rng,
                |rng| {
                    let mut builder = GraphBuilder::new(nodes);
                    for _ in 0..nodes * 4 {
                        let from = NodeId(rng.below(u64::from(nodes)) as u32);
                        let to = NodeId(rng.below(u64::from(nodes)) as u32);
                        let cost = Cost::from(1 + rng.below(20) as u32);
                        builder.add_edge(from, to, cost).unwrap();
                    }
                    let list: Vec<u32> = (0..nodes).collect();
                    let net = random_net(rng, &list, terminals, 1, false);
                    (builder.build(), net)
                },
            );
        }
    }

    #[test]
    #[ignore = "a long measurement: run with --release --ignored --nocapture"]
    fn measure_quality_against_the_optimum() {
        const SAMPLE: usize = 20_000;
        let mut rng = Rng::new(1981);
        let k = |rng: &mut Rng| 3 + rng.below(4) as usize;
        measure("grid, 15% blocked", SAMPLE, true, &mut rng, |rng| {
            let (grid, free) = random_grid(rng, 1, 15);
            let count = grid.cell_count() as usize;
            let terminals = k(rng);
            let net = random_net(rng, &free, terminals, 1, false);
            (grid, count, net)
        });
        measure("2-layer grid, 15% blocked", SAMPLE, true, &mut rng, |rng| {
            let (grid, free) = random_grid(rng, 2, 15);
            let count = grid.cell_count() as usize;
            let terminals = k(rng);
            let net = random_net(rng, &free, terminals, 1, false);
            (grid, count, net)
        });
        measure("symmetric graph", SAMPLE, true, &mut rng, |rng| {
            let (graph, list) = random_graph(rng, true);
            let count = list.len();
            let terminals = k(rng);
            let net = random_net(rng, &list, terminals, 1, false);
            (graph, count, net)
        });
        measure("directed graph", SAMPLE, true, &mut rng, |rng| {
            let (graph, list) = random_graph(rng, false);
            let count = list.len();
            let terminals = k(rng);
            let net = random_net(rng, &list, terminals, 1, false);
            (graph, count, net)
        });
        measure("directed fabric", SAMPLE, true, &mut rng, |rng| {
            let (graph, list) = random_fabric(rng);
            let count = list.len() + 1;
            let terminals = k(rng);
            let mut net = random_net(rng, &list, terminals, 1, false);
            net.terminals[0] = Terminal::at(NodeId(0));
            (graph, count, net)
        });
        measure(
            "grid, up to 3 access points",
            SAMPLE,
            false,
            &mut rng,
            |rng| {
                let (grid, free) = random_grid(rng, 1, 15);
                let count = grid.cell_count() as usize;
                let terminals = k(rng);
                let net = random_net(rng, &free, terminals, 3, false);
                (grid, count, net)
            },
        );
        measure(
            "directed graph, up to 3 access points",
            SAMPLE,
            false,
            &mut rng,
            |rng| {
                let (graph, list) = random_graph(rng, false);
                let count = list.len();
                let terminals = k(rng);
                let net = random_net(rng, &list, terminals, 3, false);
                (graph, count, net)
            },
        );
        measure(
            "directed graph, priced access points",
            SAMPLE,
            false,
            &mut rng,
            |rng| {
                let (graph, list) = random_graph(rng, false);
                let count = list.len();
                let terminals = k(rng);
                let net = random_net(rng, &list, terminals, 3, true);
                (graph, count, net)
            },
        );
    }

    // Helpers for the tests below.

    fn cost(value: u32) -> Cost {
        Cost::from(value)
    }

    fn graph(nodes: u32, edges: &[(u32, u32, u32)]) -> Graph {
        let mut builder = GraphBuilder::new(nodes);
        for &(from, to, value) in edges {
            builder
                .add_edge(NodeId(from), NodeId(to), cost(value))
                .unwrap();
        }
        builder.build()
    }

    fn net_of(terminals: Vec<Terminal<NodeId>>) -> Net<NodeId> {
        Net { id: NET, terminals }
    }

    /// A net with one free access point per terminal.
    fn at(nodes: &[u32]) -> Net<NodeId> {
        net_of(
            nodes
                .iter()
                .map(|&node| Terminal::at(NodeId(node)))
                .collect(),
        )
    }

    /// A terminal with the given free access points.
    fn any_of(nodes: &[u32]) -> Terminal<NodeId> {
        priced(&nodes.iter().map(|&node| (node, 0)).collect::<Vec<_>>())
    }

    /// A terminal with the given access points and costs.
    fn priced(points: &[(u32, u32)]) -> Terminal<NodeId> {
        let points = points
            .iter()
            .map(|&(node, value)| AccessPoint {
                node: NodeId(node),
                cost: cost(value),
            })
            .collect();
        Terminal::new(points).unwrap()
    }

    fn edges_of(tree: &SteinerTree<NodeId>) -> Vec<(u32, u32)> {
        let mut edges: Vec<_> = tree
            .route
            .edges()
            .iter()
            .map(|edge| (edge.from.0, edge.to.0))
            .collect();
        edges.sort_unstable();
        edges
    }

    /// Everything a returned tree has to satisfy, checked without the code that built it.
    fn check<S: RoutingSpace>(
        space: &S,
        net: &Net<S::Node>,
        tree: &SteinerTree<S::Node>,
    ) -> Result<(), TestCaseError> {
        prop_assert_eq!(tree.route.validate(net), Ok(()));
        if !net.needs_routing() {
            prop_assert!(tree.route.is_empty());
            prop_assert_eq!(tree.cost, Cost::ZERO);
            return Ok(());
        }

        let root = tree.route.root();
        prop_assert!(root.is_some());
        let root = root.unwrap();
        let driver = net.terminals[0].access_points();
        let root_cost = driver
            .iter()
            .filter(|point| point.node == root)
            .map(|point| point.cost)
            .min();
        prop_assert!(root_cost.is_some(), "the root is not on the driver");

        let mut sum = Cost::ZERO;
        let mut steps = Vec::new();
        let mut starts = Vec::new();
        for edge in tree.route.edges() {
            steps.clear();
            space.expand(net.id, edge.from, &mut steps);
            let step = steps.iter().find(|step| step.to == edge.to);
            prop_assert!(step.is_some(), "{:?} is not a step", edge);
            sum = sum + step.unwrap().cost;
            starts.push(edge.from);
        }
        prop_assert_eq!(tree.step_cost, sum);

        let nodes: Vec<S::Node> = tree.route.nodes().collect();
        let mut access = root_cost.unwrap();
        for terminal in &net.terminals[1..] {
            let cheapest = terminal
                .access_points()
                .iter()
                .filter(|point| nodes.contains(&point.node))
                .map(|point| point.cost)
                .min();
            access = access + cheapest.unwrap();
        }
        prop_assert_eq!(tree.access_cost, access);
        prop_assert_eq!(tree.cost, sum + access);

        for &node in &nodes[1..] {
            let holds_a_terminal = net.terminals[1..].iter().any(|terminal| {
                let points = terminal.access_points();
                points.iter().any(|point| point.node == node)
            });
            prop_assert!(
                starts.contains(&node) || holds_a_terminal,
                "{:?} is a leaf that serves no terminal",
                node
            );
        }
        Ok(())
    }

    /// Builds a tree with each construction, checks them, and returns their costs in order
    /// of growth, then [`kmb_tree`] with `options`.
    fn both<S: RoutingSpace>(
        space: &S,
        net: &Net<S::Node>,
        options: &SteinerOptions,
    ) -> (SteinerTree<S::Node>, SteinerTree<S::Node>) {
        let mut search = Search::new(space);
        let growth = takahashi_matsuyama(&mut search, space, net).unwrap();
        check(space, net, &growth).unwrap();
        let kmb = kmb_tree(&mut search, space, net, options).unwrap();
        check(space, net, &kmb).unwrap();
        (growth, kmb)
    }

    fn unrefined() -> SteinerOptions {
        SteinerOptions {
            refine_searches_per_terminal: 0,
            ..SteinerOptions::default()
        }
    }

    // Trees checked by hand.

    #[test]
    fn three_terminals_join_in_a_t() {
        // S . . . a
        // . . . . .
        // . . . . .
        // . . b . .
        let grid = Grid::new(5, 4, 1).unwrap();
        let node = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let net = at(&[node(0, 0), node(4, 0), node(2, 3)]);
        assert_eq!(exact_cost(&grid, 20, &net), Some(7.0));

        let (growth, kmb) = both(&grid, &net, &SteinerOptions::default());
        // Star connection from the driver would pay 4 + 5 = 9.
        assert_eq!(kmb.cost, cost(7));
        assert_eq!(growth.cost, cost(7));
        // The Steiner point is on no terminal.
        assert!(kmb.route.contains(NodeId(node(2, 0))));
    }

    #[test]
    fn a_steiner_point_beats_separate_paths() {
        // Each direct edge costs 3, and through the hub at node 1 each terminal costs 2 on
        // its own. Direct edges total 9 and separate shortest paths total 6, but sharing
        // the first step totals 4.
        let space = graph(
            5,
            &[
                (0, 2, 3),
                (0, 3, 3),
                (0, 4, 3),
                (0, 1, 1),
                (1, 2, 1),
                (1, 3, 1),
                (1, 4, 1),
            ],
        );
        let net = at(&[0, 2, 3, 4]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(kmb.cost, cost(4));
        assert_eq!(edges_of(&kmb), [(0, 1), (1, 2), (1, 3), (1, 4)]);
    }

    #[test]
    fn a_terminal_on_the_way_to_another_is_passed_through() {
        let space = graph(4, &[(0, 1, 1), (1, 2, 1), (2, 3, 1)]);
        for terminals in [&[0, 1, 3], &[0, 3, 1]] {
            let (growth, kmb) = both(&space, &at(terminals), &SteinerOptions::default());
            assert_eq!(edges_of(&kmb), [(0, 1), (1, 2), (2, 3)]);
            assert_eq!(kmb.cost, cost(3));
            assert_eq!(growth.cost, cost(3));
        }
    }

    #[test]
    fn duplicate_terminals_cost_nothing_extra() {
        let space = graph(3, &[(0, 1, 1), (1, 2, 1)]);
        let net = at(&[0, 2, 2, 0, 1, 2]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(edges_of(&kmb), [(0, 1), (1, 2)]);
        assert_eq!(kmb.cost, cost(2));
    }

    #[test]
    fn terminals_on_one_node_need_only_that_node() {
        let space = graph(2, &[(0, 1, 1)]);
        let (_, kmb) = both(&space, &at(&[1, 1]), &SteinerOptions::default());
        assert_eq!(kmb.route, Route::from_root(NodeId(1)));
        assert_eq!(kmb.cost, Cost::ZERO);

        // Sharing one access point among several is enough too.
        let net = net_of(vec![any_of(&[0, 1]), any_of(&[1, 0]), any_of(&[1])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(kmb.route, Route::from_root(NodeId(1)));
    }

    #[test]
    fn nets_with_nothing_to_connect_get_an_empty_route() {
        let space = graph(2, &[]);
        let unreachable_alone = net_of(vec![priced(&[(9, 5)])]);
        for net in [at(&[]), at(&[1]), unreachable_alone] {
            let mut search = Search::new(&space);
            let options = SteinerOptions::default();
            let growth = takahashi_matsuyama(&mut search, &space, &net).unwrap();
            let kmb = kmb_tree(&mut search, &space, &net, &options).unwrap();
            for tree in [growth, kmb] {
                check(&space, &net, &tree).unwrap();
                assert_eq!(tree.route, Route::new());
                assert_eq!(tree.searches, 0);
            }
        }
    }

    // Direction.

    #[test]
    fn the_undirected_answer_does_not_exist() {
        // Ignoring direction, the closure's minimum spanning tree is 0 - 2 - 1 and costs 2.
        // Rooted at the driver it would need 2 -> 1, which is not an edge.
        let space = graph(3, &[(1, 2, 1), (0, 2, 1), (0, 1, 5)]);
        let net = at(&[0, 1, 2]);
        assert_eq!(exact_cost(&space, 3, &net), Some(6.0));
        let (growth, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(kmb.cost, cost(6));
        assert_eq!(growth.cost, cost(6));
    }

    #[test]
    fn the_driver_decides_what_can_be_connected() {
        let space = graph(3, &[(1, 2, 1), (0, 2, 1), (0, 1, 5)]);
        let mut search = Search::new(&space);
        let options = SteinerOptions::default();
        let net = at(&[1, 0, 2]);
        let error = SteinerError::UnreachableTerminal { terminal: 1 };
        assert_eq!(kmb_tree(&mut search, &space, &net, &options), Err(error));
        assert_eq!(takahashi_matsuyama(&mut search, &space, &net), Err(error));
    }

    #[test]
    fn nearest_first_can_be_lured_where_the_arborescence_is_not() {
        // Node 1 is nearer, but node 2 reaches it for free. Growth connects 1 first and pays
        // for both edges. The closure's cheapest edge into 1 comes from 2, so that tree
        // connects 2 first.
        let space = graph(3, &[(0, 1, 2), (0, 2, 3), (2, 1, 0)]);
        let net = at(&[0, 1, 2]);
        assert_eq!(exact_cost(&space, 3, &net), Some(3.0));
        let (growth, kmb) = both(&space, &net, &unrefined());
        assert_eq!(growth.cost, cost(5));
        assert_eq!(kmb.cost, cost(3));
        assert_eq!(edges_of(&kmb), [(0, 2), (2, 1)]);
    }

    #[test]
    fn a_cycle_of_cheapest_edges_is_resolved() {
        // Each of 1 and 2 is cheapest to enter from the other, so picking the cheapest edge
        // into each node gives a cycle that the driver does not reach.
        let space = graph(3, &[(0, 1, 10), (0, 2, 12), (1, 2, 1), (2, 1, 1)]);
        let net = at(&[0, 1, 2]);
        let (_, kmb) = both(&space, &net, &unrefined());
        assert_eq!(kmb.cost, cost(11));
        assert_eq!(edges_of(&kmb), [(0, 1), (1, 2)]);
    }

    #[test]
    fn directed_spaces_have_no_constant_factor() {
        // Each sink has a private hub at 9, and all can share one at 10. Every shortest
        // path avoids the shared hub, and so does every reconnection of a single sink, so
        // the result is 45 against an optimum of 10. With `n` sinks the ratio tends to `n`.
        const SINKS: u32 = 5;
        let shared = 2 * SINKS + 1;
        let mut edges = vec![(0, shared, 10)];
        for sink in 1..=SINKS {
            let (hub, end) = (sink, SINKS + sink);
            edges.extend([(0, hub, 9), (hub, end, 0), (shared, end, 0)]);
        }
        let space = graph(shared + 1, &edges);
        let terminals: Vec<u32> = core::iter::once(0)
            .chain((1..=SINKS).map(|sink| SINKS + sink))
            .collect();
        let net = at(&terminals);

        assert_eq!(exact_cost(&space, shared as usize + 1, &net), Some(10.0));
        let (growth, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(growth.cost, cost(45));
        assert_eq!(kmb.cost, cost(45));
        // That is above the factor of 2 of symmetric spaces, and within the factor that does
        // hold here, one per sink.
        assert!(kmb.cost.get() > 2.0 * 10.0);
        assert!(kmb.cost.get() <= f64::from(SINKS) * 10.0);
    }

    // Access points.

    #[test]
    fn the_nearest_access_point_is_enough() {
        let space = graph(4, &[(0, 1, 1), (1, 2, 1), (2, 3, 1)]);
        let net = net_of(vec![any_of(&[0]), any_of(&[3, 1])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(edges_of(&kmb), [(0, 1)]);
    }

    #[test]
    fn an_access_point_on_the_way_to_another_terminal_costs_nothing() {
        let space = graph(5, &[(0, 1, 1), (1, 2, 1), (2, 3, 1), (0, 4, 1)]);
        let net = net_of(vec![any_of(&[0]), any_of(&[3]), any_of(&[1, 4])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(edges_of(&kmb), [(0, 1), (1, 2), (2, 3)]);
        assert_eq!(kmb.cost, cost(3));
    }

    #[test]
    fn a_later_path_makes_an_earlier_access_point_redundant() {
        // Growth takes node 1 for the first terminal, but the way to the second terminal
        // passes node 2, which serves the first terminal as well. So the branch to 1 goes.
        let space = graph(4, &[(0, 1, 1), (0, 2, 2), (2, 3, 2)]);
        let net = net_of(vec![any_of(&[0]), any_of(&[1, 2]), any_of(&[3])]);
        assert_eq!(exact_cost(&space, 4, &net), Some(4.0));
        let (growth, _) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(growth.cost, cost(4));
        assert_eq!(edges_of(&growth), [(0, 2), (2, 3)]);
    }

    #[test]
    fn the_access_point_to_arrive_at_is_the_one_that_leads_on() {
        // The first terminal's nearest access point is node 1, at 1. From there each sink
        // costs 3, for 7 in all. Node 2 is dearer at 3, but serves both sinks for 1 each.
        let space = graph(
            5,
            &[
                (0, 1, 1),
                (1, 3, 3),
                (1, 4, 3),
                (0, 2, 3),
                (2, 3, 1),
                (2, 4, 1),
            ],
        );
        let net = net_of(vec![
            any_of(&[0]),
            any_of(&[1, 2]),
            any_of(&[3]),
            any_of(&[4]),
        ]);
        assert_eq!(exact_cost(&space, 5, &net), Some(5.0));
        let (growth, kmb) = both(&space, &net, &unrefined());
        assert_eq!(growth.cost, cost(7));
        assert_eq!(kmb.cost, cost(5));
        assert_eq!(edges_of(&kmb), [(0, 2), (2, 3), (2, 4)]);
    }

    #[test]
    fn access_costs_count_towards_the_choice_and_the_total() {
        let space = graph(3, &[(0, 1, 1), (0, 2, 3)]);

        // 1 + 5 loses to 3 + 0.
        let net = net_of(vec![any_of(&[0]), priced(&[(1, 5), (2, 0)])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(edges_of(&kmb), [(0, 2)]);
        assert_eq!((kmb.step_cost, kmb.access_cost), (cost(3), cost(0)));

        // 1 + 1 beats 3 + 0.
        let net = net_of(vec![any_of(&[0]), priced(&[(1, 1), (2, 0)])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(edges_of(&kmb), [(0, 1)]);
        assert_eq!((kmb.step_cost, kmb.access_cost), (cost(1), cost(1)));
        assert_eq!(kmb.cost, cost(2));

        // The driver pays for the access point at the root.
        let net = net_of(vec![priced(&[(0, 7)]), any_of(&[1])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!((kmb.step_cost, kmb.access_cost), (cost(1), cost(7)));
    }

    #[test]
    fn a_dear_access_point_already_on_the_tree_can_lose_to_extending_it() {
        let space = graph(3, &[(0, 1, 1), (1, 2, 1)]);
        let net = net_of(vec![any_of(&[0]), any_of(&[1]), priced(&[(1, 9), (2, 0)])]);
        assert_eq!(exact_cost(&space, 3, &net), Some(2.0));
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(edges_of(&kmb), [(0, 1), (1, 2)]);
        assert_eq!(kmb.cost, cost(2));
    }

    #[test]
    fn the_driver_can_start_from_whichever_access_point_does_best() {
        let space = graph(4, &[(0, 2, 5), (1, 2, 1), (1, 3, 1)]);

        let net = net_of(vec![any_of(&[0, 1]), any_of(&[2]), any_of(&[3])]);
        let (growth, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(growth.route.root(), Some(NodeId(1)));
        assert_eq!(kmb.route.root(), Some(NodeId(1)));
        assert_eq!(kmb.cost, cost(2));

        // Only node 1 reaches everything, even though node 0 comes first and is free.
        let net = net_of(vec![priced(&[(0, 0), (1, 50)]), any_of(&[2]), any_of(&[3])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(kmb.route.root(), Some(NodeId(1)));
        assert_eq!(kmb.cost, cost(52));

        // A price can decide it the other way, with one terminal to connect.
        let net = net_of(vec![priced(&[(0, 0), (1, 50)]), any_of(&[2])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(kmb.route.root(), Some(NodeId(0)));
        assert_eq!(kmb.cost, cost(5));
    }

    // Unreachable terminals.

    #[test]
    fn reports_the_lowest_unreachable_terminal() {
        let space = graph(5, &[(0, 1, 1), (3, 0, 1)]);
        let mut search = Search::new(&space);
        let net = at(&[0, 1, 2, 3, 4]);
        let error = SteinerError::UnreachableTerminal { terminal: 2 };
        assert_eq!(takahashi_matsuyama(&mut search, &space, &net), Err(error));
        let options = SteinerOptions::default();
        assert_eq!(kmb_tree(&mut search, &space, &net, &options), Err(error));
        assert_eq!(
            error.to_string(),
            "terminal 2 cannot be reached from the driver"
        );
    }

    #[test]
    fn an_unreachable_access_point_does_not_matter_if_another_is_reachable() {
        let space = graph(3, &[(0, 1, 1)]);
        let net = net_of(vec![any_of(&[0]), any_of(&[2, 1])]);
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(edges_of(&kmb), [(0, 1)]);
    }

    #[test]
    fn terminals_need_a_common_root() {
        // Each sink can be reached from some access point of the driver, but no route can
        // start from both. Both roots leave one sink out, so the first root decides.
        let space = graph(4, &[(0, 2, 1), (1, 3, 1)]);
        let mut search = Search::new(&space);
        let net = net_of(vec![any_of(&[0, 1]), any_of(&[2]), any_of(&[3])]);
        let options = SteinerOptions::default();
        assert_eq!(
            kmb_tree(&mut search, &space, &net, &options),
            Err(SteinerError::UnreachableTerminal { terminal: 2 })
        );
    }

    #[test]
    fn the_root_that_reaches_the_most_decides_which_terminal_is_reported() {
        // Node 0 reaches nothing. Node 1 reaches all but the last terminal.
        let space = graph(4, &[(1, 2, 1)]);
        let mut search = Search::new(&space);
        let net = net_of(vec![any_of(&[0, 1]), any_of(&[2]), any_of(&[3])]);
        assert_eq!(
            takahashi_matsuyama(&mut search, &space, &net),
            Err(SteinerError::UnreachableTerminal { terminal: 2 })
        );
    }

    #[test]
    fn blocked_cells_cut_terminals_off() {
        let mut grid = Grid::new(3, 3, 1).unwrap();
        for (x, y) in [(1, 0), (1, 1), (1, 2)] {
            grid.set_blocked(x, y, 0, true).unwrap();
        }
        let left = grid.node_at(0, 2, 0).unwrap().0;
        let right = grid.node_at(2, 2, 0).unwrap().0;
        let mut search = Search::new(&grid);
        assert_eq!(
            takahashi_matsuyama(&mut search, &grid, &at(&[0, left, right])),
            Err(SteinerError::UnreachableTerminal { terminal: 2 })
        );

        // A terminal on a blocked cell is cut off as well, wherever the rest of the net is.
        let blocked = grid.node_at(1, 1, 0).unwrap().0;
        assert_eq!(
            takahashi_matsuyama(&mut search, &grid, &at(&[0, left, blocked])),
            Err(SteinerError::UnreachableTerminal { terminal: 2 })
        );
    }

    // Refinement.

    #[test]
    fn refinement_reconnects_what_growth_got_wrong() {
        let space = graph(3, &[(0, 1, 2), (0, 2, 3), (2, 1, 0)]);
        let net = at(&[0, 1, 2]);
        let mut search = Search::new(&space);
        let growth = takahashi_matsuyama(&mut search, &space, &net).unwrap();
        assert_eq!(growth.cost, cost(5));
        let options = SteinerOptions::default();
        let refined = refine_tree(&mut search, &space, &net, &growth.route, &options).unwrap();
        check(&space, &net, &refined).unwrap();
        assert_eq!(refined.cost, cost(3));
    }

    #[test]
    fn refinement_stops_where_the_budget_says() {
        let space = graph(3, &[(0, 1, 2), (0, 2, 3), (2, 1, 0)]);
        let net = at(&[0, 1, 2]);
        let mut search = Search::new(&space);
        let growth = takahashi_matsuyama(&mut search, &space, &net).unwrap();
        let refined = refine_tree(&mut search, &space, &net, &growth.route, &unrefined()).unwrap();
        assert_eq!(refined.route, growth.route);
        assert_eq!(refined.searches, 0);
    }

    #[test]
    fn refinement_prunes_a_route_from_elsewhere_whatever_its_edge_order() {
        let space = graph(4, &[(0, 1, 1), (1, 2, 1), (1, 3, 1)]);
        let net = at(&[0, 2]);
        let edge = |from, to| Edge {
            from: NodeId(from),
            to: NodeId(to),
        };
        let route = Route::from_edges(NodeId(0), vec![edge(1, 3), edge(1, 2), edge(0, 1)]);
        let mut search = Search::new(&space);
        let options = SteinerOptions::default();
        let refined = refine_tree(&mut search, &space, &net, &route, &options).unwrap();
        check(&space, &net, &refined).unwrap();
        assert_eq!(edges_of(&refined), [(0, 1), (1, 2)]);
    }

    #[test]
    fn refinement_rejects_routes_it_cannot_work_with() {
        let space = graph(3, &[(0, 1, 1), (1, 2, 1)]);
        let mut search = Search::new(&space);
        let options = SteinerOptions::default();
        let edge = |from, to| Edge {
            from: NodeId(from),
            to: NodeId(to),
        };
        let net = at(&[0, 2]);

        let missing = RouteError::MissingTerminal { terminal: 0 };
        assert_eq!(
            refine_tree(&mut search, &space, &net, &Route::new(), &options),
            Err(RefineError::InvalidRoute(missing))
        );

        let route = Route::from_edges(NodeId(0), vec![edge(0, 1)]);
        assert_eq!(
            refine_tree(&mut search, &space, &net, &route, &options),
            Err(RefineError::InvalidRoute(RouteError::MissingTerminal {
                terminal: 1
            }))
        );

        // Connects both terminals, but starts from the wrong end.
        let net = at(&[1, 2]);
        let route = Route::from_edges(NodeId(0), vec![edge(0, 1), edge(1, 2)]);
        let error = refine_tree(&mut search, &space, &net, &route, &options).unwrap_err();
        assert_eq!(error, RefineError::RootNotDriver { root: NodeId(0) });
        assert!(error.to_string().contains("NodeId(0)"));

        let net = at(&[0, 2]);
        let route = Route::from_edges(NodeId(0), vec![edge(0, 2)]);
        let error = refine_tree(&mut search, &space, &net, &route, &options).unwrap_err();
        assert_eq!(
            error,
            RefineError::NotAStep {
                from: NodeId(0),
                to: NodeId(2)
            }
        );
        assert!(error.to_string().contains("no step"));

        // Nothing to connect means nothing to keep.
        let net = at(&[0]);
        let refined = refine_tree(&mut search, &space, &net, &route, &options).unwrap();
        assert_eq!(refined.route, Route::new());
    }

    // Other spaces.

    /// A grid with a surcharge on some cells, as a negotiating engine would price them.
    struct Congested<'a> {
        grid: &'a Grid,
        surcharge: HashMap<NodeId, Cost>,
    }

    impl RoutingSpace for Congested<'_> {
        type Node = NodeId;
        type Resource = NodeId;
        type Slots = <Grid as RoutingSpace>::Slots;

        fn new_slots(&self) -> Self::Slots {
            self.grid.new_slots()
        }

        fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
            let start = out.len();
            self.grid.expand(net, from, out);
            for step in &mut out[start..] {
                let extra = self.surcharge.get(&step.to).copied();
                step.cost = step.cost + extra.unwrap_or(Cost::ZERO);
            }
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
    fn surcharges_move_the_tree() {
        let grid = Grid::new(5, 4, 1).unwrap();
        let node = |x, y| grid.node_at(x, y, 0).unwrap();
        let net = at(&[node(0, 0).0, node(4, 0).0, node(2, 3).0]);
        let surcharge = (0..5).map(|x| (node(x, 0), cost(100))).collect();
        // The whole first row is dear, including the terminal at its end, which any tree has
        // to pay for once. So the cheapest tree runs along the second row instead.
        let space = Congested {
            grid: &grid,
            surcharge,
        };
        assert_eq!(
            exact_cost(&space, 20, &net),
            Some(1.0 + 4.0 + 2.0 + 1.0 + 100.0)
        );
        let (_, kmb) = both(&space, &net, &SteinerOptions::default());
        assert_eq!(kmb.cost, cost(108));
        assert!(kmb.route.contains(node(2, 1)));
        assert!(!kmb.route.contains(node(2, 0)));
    }

    /// A row of cells generated on demand, whose nodes have no order and no dense index.
    struct Lazy;

    impl RoutingSpace for Lazy {
        type Node = (i32, char);
        type Resource = (i32, char);
        type Slots = InternedSlots<(i32, char)>;

        fn new_slots(&self) -> Self::Slots {
            InternedSlots::new()
        }

        fn expand(&self, _: NetId, (x, layer): (i32, char), out: &mut Vec<Step<(i32, char)>>) {
            for to in [x - 1, x + 1] {
                if (0..8).contains(&to) {
                    out.push(Step {
                        to: (to, layer),
                        cost: Cost::ONE,
                    });
                }
            }
            let other = if layer == 'a' { 'b' } else { 'a' };
            out.push(Step {
                to: (x, other),
                cost: Cost::ONE,
            });
        }

        fn lower_bound(&self, _: NetId, from: (i32, char), to: (i32, char)) -> Cost {
            let layers = u32::from(from.1 != to.1);
            Cost::from(from.0.abs_diff(to.0) + layers)
        }

        fn resources(&self, _: NetId, _: (i32, char), to: (i32, char), out: &mut Vec<(i32, char)>) {
            out.push(to);
        }

        fn capacity(&self, _: (i32, char)) -> u32 {
            1
        }
    }

    #[test]
    fn nodes_only_need_what_a_space_requires() {
        let net = Net {
            id: NET,
            terminals: vec![
                Terminal::at((0, 'a')),
                Terminal::at((5, 'a')),
                Terminal::at((5, 'b')),
                Terminal::at((2, 'b')),
            ],
        };
        let (_, kmb) = both(&Lazy, &net, &SteinerOptions::default());
        // Along layer a to 2, a via, along layer b to 5, and a via: 2 + 1 + 3 + 1.
        assert_eq!(kmb.cost, cost(7));
    }

    // Determinism and reuse.

    #[test]
    fn repeated_calls_keep_the_search_buffers_and_the_answer() {
        let grid = Grid::new(12, 12, 1).unwrap();
        let net = at(&[0, 11, 132, 143, 66, 40]);
        let options = SteinerOptions::default();
        let mut search = Search::new(&grid);
        let first = kmb_tree(&mut search, &grid, &net, &options).unwrap();
        let grown = format!("{search:?}");
        for _ in 0..5 {
            assert_eq!(
                kmb_tree(&mut search, &grid, &net, &options),
                Ok(first.clone())
            );
            // The slot count is the part of the state that the search reports.
            assert_eq!(
                format!("{search:?}").split(',').next(),
                grown.split(',').next()
            );
        }
        let mut fresh = Search::new(&grid);
        assert_eq!(kmb_tree(&mut fresh, &grid, &net, &options), Ok(first));
    }

    // The arborescence on its own.

    fn closure_edges(edges: &[(usize, usize, f64)]) -> Vec<ClosureEdge> {
        edges
            .iter()
            .map(|&(from, to, weight)| ClosureEdge { from, to, weight })
            .collect()
    }

    fn chosen_pairs(edges: &[ClosureEdge], chosen: &[usize]) -> Vec<(usize, usize)> {
        let mut pairs: Vec<_> = chosen
            .iter()
            .map(|&edge| (edges[edge].from, edges[edge].to))
            .collect();
        pairs.sort_unstable();
        pairs
    }

    #[test]
    fn arborescence_of_a_tree_is_that_tree() {
        let edges = closure_edges(&[(0, 1, 5.0), (1, 2, 5.0)]);
        let chosen = min_arborescence(3, 0, &edges).unwrap();
        assert_eq!(chosen_pairs(&edges, &chosen), [(0, 1), (1, 2)]);
    }

    #[test]
    fn arborescence_needs_every_node_reached() {
        let edges = closure_edges(&[(1, 2, 1.0), (2, 1, 1.0)]);
        assert_eq!(min_arborescence(3, 0, &edges), None);
        assert_eq!(min_arborescence(1, 0, &[]), Some(vec![]));
    }

    #[test]
    fn arborescence_resolves_nested_cycles() {
        // 1 and 2 form a cycle, which forms a larger one with 3 once contracted.
        let edges = closure_edges(&[
            (0, 1, 100.0),
            (0, 3, 50.0),
            (1, 2, 1.0),
            (2, 1, 1.0),
            (3, 1, 5.0),
            (2, 3, 4.0),
        ]);
        let chosen = min_arborescence(4, 0, &edges).unwrap();
        // Enter at 3 for 50, on to 1 for 5, and on to 2 for 1.
        assert_eq!(chosen_pairs(&edges, &chosen), [(0, 3), (1, 2), (3, 1)]);
    }

    #[test]
    fn arborescence_breaks_ties_by_edge_order() {
        let edges = closure_edges(&[(0, 2, 1.0), (1, 2, 1.0), (0, 1, 1.0)]);
        let chosen = min_arborescence(3, 0, &edges).unwrap();
        assert_eq!(chosen_pairs(&edges, &chosen), [(0, 1), (0, 2)]);
    }

    #[test]
    fn arborescence_survives_a_long_chain_of_contractions() {
        // Each contraction merges one node, for the deepest possible stack of levels.
        const NODES: usize = 400;
        let mut list = vec![(0, 1, 1e6)];
        for node in 1..NODES - 1 {
            list.push((node, node + 1, 1.0));
            list.push((node + 1, node, (node + 1) as f64 / 1e3));
        }
        let edges = closure_edges(&list);
        let chosen = min_arborescence(NODES, 0, &edges).unwrap();
        assert_eq!(chosen.len(), NODES - 1);
        assert!(chosen.contains(&0));
    }

    // Against the optimum, on a sample fixed by the seed.

    /// The mean ratios to the optimum of growth and of [`kmb_tree`] with default options.
    fn mean_ratios<S: RoutingSpace<Node = NodeId>>(
        sample: usize,
        rng: &mut Rng,
        mut make: impl FnMut(&mut Rng) -> (S, usize, Net<NodeId>),
    ) -> (f64, f64) {
        let (mut growth_total, mut kmb_total) = (0.0, 0.0);
        let mut done = 0;
        while done < sample {
            let (space, nodes, net) = make(rng);
            let Some(optimum) = exact_cost(&space, nodes, &net) else {
                continue;
            };
            done += 1;
            let (growth, kmb) = both(&space, &net, &SteinerOptions::default());
            assert!(kmb.cost <= growth.cost);
            if optimum > 0.0 {
                growth_total += growth.cost.get() / optimum;
                kmb_total += kmb.cost.get() / optimum;
            } else {
                growth_total += 1.0;
                kmb_total += 1.0;
            }
        }
        (growth_total / sample as f64, kmb_total / sample as f64)
    }

    /// Guards the measured gains: see `measure_quality_against_the_optimum` for more.
    #[test]
    fn beats_growth_on_average_and_stays_near_the_optimum() {
        let mut rng = Rng::new(7);
        let (growth, kmb) = mean_ratios(250, &mut rng, |rng| {
            let (grid, free) = random_grid(rng, 1, 15);
            let count = grid.cell_count() as usize;
            let terminals = 3 + rng.below(4) as usize;
            let net = random_net(rng, &free, terminals, 1, false);
            (grid, count, net)
        });
        assert!(kmb < growth, "{kmb} against {growth}");
        assert!(kmb < 1.03, "{kmb}");

        let (growth, kmb) = mean_ratios(250, &mut rng, |rng| {
            let (graph, list) = random_graph(rng, false);
            let count = list.len();
            let terminals = 3 + rng.below(4) as usize;
            let net = random_net(rng, &list, terminals, 1, false);
            (graph, count, net)
        });
        assert!(kmb + 0.01 < growth, "{kmb} against {growth}");
        assert!(kmb < 1.02, "{kmb}");
    }

    // Properties.

    /// A random graph and net, small enough for the exact answer.
    #[derive(Clone, Debug)]
    struct GraphCase {
        nodes: u32,
        edges: Vec<(u32, u32, u32)>,
        terminals: Vec<Vec<(u32, u32)>>,
    }

    impl GraphCase {
        fn graph(&self) -> Graph {
            graph(self.nodes, &self.edges)
        }

        fn net(&self) -> Net<NodeId> {
            net_of(self.terminals.iter().map(|list| priced(list)).collect())
        }
    }

    fn terminals_strategy(
        nodes: u32,
        count: core::ops::Range<usize>,
        points: core::ops::RangeInclusive<usize>,
        price: u32,
    ) -> impl Strategy<Value = Vec<Vec<(u32, u32)>>> {
        vec(vec((0..nodes, 0..=price), points), count)
    }

    fn graph_case(
        max_terminals: usize,
        max_points: usize,
        price: u32,
    ) -> impl Strategy<Value = GraphCase> {
        (2u32..=12)
            .prop_flat_map(move |nodes| {
                (
                    Just(nodes),
                    vec((0..nodes, 0..nodes, 0u32..=20), 0..40),
                    terminals_strategy(nodes, 0..max_terminals + 1, 1..=max_points, price),
                    proptest::bool::weighted(0.75),
                )
            })
            .prop_map(|(nodes, mut edges, terminals, connected)| {
                // Sparse random graphs rarely connect a whole net, so most cases get a dear
                // ring through every node, leaving the rest to test unreachable terminals.
                if connected {
                    edges.extend((0..nodes).map(|node| (node, (node + 1) % nodes, 30)));
                }
                GraphCase {
                    nodes,
                    edges,
                    terminals,
                }
            })
    }

    fn options_strategy() -> impl Strategy<Value = SteinerOptions> {
        (
            prop_oneof![Just(0), Just(1), Just(2), Just(8), Just(usize::MAX)],
            prop_oneof![Just(0), Just(1), Just(8), Just(64)],
        )
            .prop_map(|(closure_neighbours, refine_searches_per_terminal)| {
                SteinerOptions {
                    closure_neighbours,
                    refine_searches_per_terminal,
                }
            })
    }

    /// Checks the promises that hold for every net: that a tree exists exactly when an
    /// optimum does, that it is valid, and how its cost relates to growth and the optimum.
    fn check_against_optimum<S: RoutingSpace<Node = NodeId>>(
        space: &S,
        nodes: usize,
        net: &Net<NodeId>,
        options: &SteinerOptions,
    ) -> Result<(), TestCaseError> {
        let mut search = Search::new(space);
        let kmb = kmb_tree(&mut search, space, net, options);
        let growth = takahashi_matsuyama(&mut search, space, net);
        if !net.needs_routing() {
            check(space, net, &kmb.unwrap())?;
            check(space, net, &growth.unwrap())?;
            return Ok(());
        }
        let Some(optimum) = exact_cost(space, nodes, net) else {
            let names_a_sink = matches!(
                kmb,
                Err(SteinerError::UnreachableTerminal { terminal }) if terminal >= 1
            );
            prop_assert!(names_a_sink);
            prop_assert_eq!(growth.err(), kmb.err());
            return Ok(());
        };
        let (kmb, growth) = (kmb.unwrap(), growth.unwrap());
        check(space, net, &kmb)?;
        check(space, net, &growth)?;

        // Costs here can be fractions that were added up in different orders.
        let slack = 1.0 + 1e-9;
        prop_assert!(kmb.cost.get() * slack >= optimum, "below the optimum");
        prop_assert!(kmb.cost <= growth.cost, "worse than growth");
        let sinks = (net.terminals.len() - 1) as f64;
        prop_assert!(
            growth.cost.get() <= sinks * optimum * slack,
            "beyond the bound"
        );
        if net.terminals.len() == 2 {
            prop_assert!(kmb.cost.get() <= optimum * slack, "one sink must be exact");
        }
        Ok(())
    }

    /// Sums every subset of edges that connects a net, which is correct because a cheapest
    /// connecting subset is a tree and costs are not negative. Far too slow for anything
    /// but checking `exact_cost`.
    fn subset_optimum(case: &GraphCase) -> Option<u32> {
        let mut best = None;
        for subset in 0u32..1 << case.edges.len() {
            let chosen = |edge: usize| subset & (1 << edge) != 0;
            let steps: u32 = (0..case.edges.len())
                .filter(|&edge| chosen(edge))
                .map(|edge| case.edges[edge].2)
                .sum();
            for &(root, root_price) in &case.terminals[0] {
                let mut reached = vec![false; case.nodes as usize];
                reached[root as usize] = true;
                loop {
                    let mut changed = false;
                    for (edge, &(from, to, _)) in case.edges.iter().enumerate() {
                        if chosen(edge) && reached[from as usize] && !reached[to as usize] {
                            reached[to as usize] = true;
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
                let access: Option<u32> = case.terminals[1..]
                    .iter()
                    .map(|terminal| {
                        terminal
                            .iter()
                            .filter(|&&(node, _)| reached[node as usize])
                            .map(|&(_, price)| price)
                            .min()
                    })
                    .sum();
                if let Some(access) = access {
                    let total = steps + root_price + access;
                    if best.is_none_or(|best| total < best) {
                        best = Some(total);
                    }
                }
            }
        }
        best
    }

    /// The weight of the cheapest arborescence, by trying every choice of parents.
    fn arborescence_optimum(nodes: usize, edges: &[ClosureEdge]) -> Option<f64> {
        fn choose(
            node: usize,
            parents: &mut Vec<usize>,
            nodes: usize,
            edges: &[ClosureEdge],
        ) -> Option<f64> {
            if node == nodes {
                // Every node has to lead to the root, without going round in circles.
                let reaches_root = (1..nodes).all(|start| {
                    let mut at = start;
                    for _ in 0..nodes {
                        if at != 0 {
                            at = edges[parents[at - 1]].from;
                        }
                    }
                    at == 0
                });
                return reaches_root.then(|| parents.iter().map(|&edge| edges[edge].weight).sum());
            }
            let mut best: Option<f64> = None;
            for (index, edge) in edges.iter().enumerate() {
                if edge.to == node {
                    parents.push(index);
                    if let Some(total) = choose(node + 1, parents, nodes, edges) {
                        best = Some(best.map_or(total, |best| best.min(total)));
                    }
                    parents.pop();
                }
            }
            best
        }
        choose(1, &mut Vec::new(), nodes, edges)
    }

    fn any_grid() -> impl Strategy<Value = (Grid, Net<NodeId>)> {
        (
            (1u32..=4, 1u32..=4, 1u32..=2),
            prop_oneof![
                Just(Connectivity::Orthogonal),
                Just(Connectivity::Octilinear)
            ],
            vec(proptest::bool::weighted(0.15), 32),
            terminals_strategy(32, 0..6, 1..=2, 2),
        )
            .prop_map(
                |((width, height, layers), connectivity, blocks, terminals)| {
                    let mut grid = Grid::builder(width, height, layers)
                        .connectivity(connectivity)
                        .via_cost(2.0)
                        .build()
                        .unwrap();
                    for node in 0..grid.cell_count() {
                        if blocks[node as usize] {
                            let (x, y, layer) = grid.position(NodeId(node)).unwrap();
                            grid.set_blocked(x, y, layer, true).unwrap();
                        }
                    }
                    let cells = grid.cell_count();
                    let terminals = terminals
                        .iter()
                        .map(|list| {
                            let points: Vec<_> = list
                                .iter()
                                .map(|&(node, price)| (node % cells, price))
                                .collect();
                            priced(&points)
                        })
                        .collect();
                    (grid, net_of(terminals))
                },
            )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        #[test]
        fn graph_trees_keep_their_promises(
            case in graph_case(6, 3, 3),
            options in options_strategy(),
        ) {
            let space = case.graph();
            check_against_optimum(&space, case.nodes as usize, &case.net(), &options)?;
        }

        #[test]
        fn grid_trees_keep_their_promises(
            (grid, net) in any_grid(),
            options in options_strategy(),
        ) {
            check_against_optimum(&grid, grid.cell_count() as usize, &net, &options)?;
        }

        /// The optimum the other tests rely on, against plain enumeration.
        #[test]
        fn exact_cost_matches_enumeration(
            nodes in 2u32..=6,
            edges in vec((0u32..6, 0u32..6, 0u32..=9), 0..=9),
            terminals in terminals_strategy(6, 2..5, 1..=2, 3),
        ) {
            let fit = |node: u32| node % nodes;
            let case = GraphCase {
                nodes,
                edges: edges
                    .iter()
                    .map(|&(from, to, value)| (fit(from), fit(to), value))
                    .collect(),
                terminals: terminals
                    .iter()
                    .map(|list| list.iter().map(|&(node, price)| (fit(node), price)).collect())
                    .collect(),
            };
            let exact = exact_cost(&case.graph(), nodes as usize, &case.net());
            prop_assert_eq!(exact, subset_optimum(&case).map(f64::from));
        }

        /// Symmetric spaces with one access point per terminal keep the classic bound.
        #[test]
        fn symmetric_spaces_stay_within_the_classic_factor(
            case in graph_case(6, 1, 0),
            options in options_strategy(),
        ) {
            let mut symmetric = case.clone();
            symmetric.edges.extend(case.edges.iter().map(|&(a, b, c)| (b, a, c)));
            let space = symmetric.graph();
            let net = symmetric.net();
            let exact = exact_cost(&space, case.nodes as usize, &net);
            if let (Some(optimum), true) = (exact, net.needs_routing()) {
                let (_, kmb) = both(&space, &net, &options);
                let terminals = net.terminals.len() as f64;
                prop_assert!(kmb.cost.get() <= 2.0 * (1.0 - 1.0 / terminals) * optimum);
            }
        }

        /// With one access point per terminal, the tree costs no more than the closure's
        /// arborescence, whichever closure was asked for.
        #[test]
        fn trees_cost_no_more_than_the_closure_arborescence(
            case in graph_case(6, 1, 0),
            neighbours in prop_oneof![Just(0), Just(1), Just(2), Just(usize::MAX)],
        ) {
            let space = case.graph();
            let net = case.net();
            prop_assume!(net.needs_routing());
            prop_assume!(exact_cost(&space, case.nodes as usize, &net).is_some());
            let mut search = Search::new(&space);
            let root = net.terminals[0].access_points()[0].node;
            let mut router = Router::new(&mut search, &space, &net);
            let (edges, _) = router.closure(root, neighbours);
            let chosen = min_arborescence(net.terminals.len(), 0, &edges).unwrap();
            let weight: f64 = chosen.iter().map(|&edge| edges[edge].weight).sum();
            let options = SteinerOptions { closure_neighbours: neighbours, ..unrefined() };
            let (_, kmb) = both(&space, &net, &options);
            prop_assert!(kmb.cost.get() <= weight);
        }

        #[test]
        fn arborescences_match_enumeration(
            nodes in 1usize..=5,
            edges in vec((0usize..5, 1usize..5, 0u32..=9), 0..14),
        ) {
            let edges: Vec<ClosureEdge> = edges
                .iter()
                .map(|&(from, to, weight)| ClosureEdge {
                    from: from % nodes,
                    to: if nodes > 1 { 1 + (to - 1) % (nodes - 1) } else { 0 },
                    weight: f64::from(weight),
                })
                .filter(|edge| edge.from != edge.to)
                .collect();
            let expected = arborescence_optimum(nodes, &edges);
            let found = min_arborescence(nodes, 0, &edges);
            prop_assert_eq!(found.is_some(), expected.is_some());
            if let (Some(found), Some(expected)) = (found, expected) {
                prop_assert_eq!(found.len(), nodes - 1);
                let total: f64 = found.iter().map(|&edge| edges[edge].weight).sum();
                prop_assert_eq!(total, expected);
                let mut parents = vec![None; nodes];
                for &edge in &found {
                    prop_assert!(parents[edges[edge].to].is_none());
                    parents[edges[edge].to] = Some(edges[edge].from);
                }
                for start in 1..nodes {
                    let mut at = start;
                    for _ in 0..nodes {
                        if let Some(parent) = parents[at] {
                            at = parent;
                        }
                    }
                    prop_assert_eq!(at, 0);
                }
            }
        }

        #[test]
        fn refinement_never_hurts_and_stays_valid(
            case in graph_case(6, 3, 3),
            options in options_strategy(),
        ) {
            let space = case.graph();
            let net = case.net();
            let mut search = Search::new(&space);
            if let Ok(growth) = takahashi_matsuyama(&mut search, &space, &net) {
                let refined = refine_tree(&mut search, &space, &net, &growth.route, &options);
                let refined = refined.unwrap();
                check(&space, &net, &refined)?;
                prop_assert!(refined.cost <= growth.cost);
            }
        }

        /// Hash maps are seeded per instance, so repeating shows they make no difference.
        #[test]
        fn the_same_input_gives_the_same_tree(
            case in graph_case(6, 3, 3),
            options in options_strategy(),
        ) {
            let space = case.graph();
            let net = case.net();
            let mut reused = Search::new(&space);
            let first = kmb_tree(&mut reused, &space, &net, &options);
            let again = kmb_tree(&mut reused, &space, &net, &options);
            prop_assert_eq!(&first, &again);
            let mut fresh = Search::new(&space);
            prop_assert_eq!(&first, &kmb_tree(&mut fresh, &space, &net, &options));
            let growth = takahashi_matsuyama(&mut reused, &space, &net);
            prop_assert_eq!(growth, takahashi_matsuyama(&mut fresh, &space, &net));
        }

        /// Sums that round or saturate may cost optimality, but nothing else.
        #[test]
        fn extreme_costs_still_give_valid_trees(
            nodes in 2u32..=8,
            edges in vec((0u32..8, 0u32..8, 0u32..6), 0..30),
            terminals in terminals_strategy(8, 2..6, 1..=2, 5),
            options in options_strategy(),
        ) {
            let stretch = |value: u32| match value {
                0 => Cost::ZERO,
                1 => Cost::new(f64::from_bits(1)).unwrap(),
                2 => Cost::new(0.1).unwrap(),
                3 => Cost::ONE,
                4 => Cost::new(1e300).unwrap(),
                _ => Cost::MAX,
            };
            let mut builder = GraphBuilder::new(nodes);
            for &(from, to, value) in &edges {
                let (from, to) = (NodeId(from % nodes), NodeId(to % nodes));
                builder.add_edge(from, to, stretch(value)).unwrap();
            }
            let space = builder.build();
            let list = terminals
                .iter()
                .map(|list| {
                    let points = list
                        .iter()
                        .map(|&(node, price)| AccessPoint {
                            node: NodeId(node % nodes),
                            cost: stretch(price),
                        })
                        .collect();
                    Terminal::new(points).unwrap()
                })
                .collect();
            let net = net_of(list);
            let mut search = Search::new(&space);
            if let Ok(tree) = kmb_tree(&mut search, &space, &net, &options) {
                check(&space, &net, &tree)?;
            }
        }
    }
}
