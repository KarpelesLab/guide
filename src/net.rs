//! Nets, terminals and routes: the problem and result model.
//!
//! Engines and the validator share these types and nothing else, so everything here is
//! plain data, and every check here works without a [`RoutingSpace`].
//!
//! # How routes are represented
//!
//! A [`Route`] is a root node and a list of directed `(from, to)` [`Edge`]s, each
//! pointing away from the root. Three things drove that choice:
//!
//! - **It can hold a wrong answer.** The validator exists to catch router bugs, so a
//!   route must be able to express a cycle or a duplicated edge for it to find. A
//!   parent-indexed form would rule some of these out, along with the means to report
//!   them.
//! - **Edges are what get verified.** [`RoutingSpace::expand`] and
//!   [`RoutingSpace::resources`] both take a `(from, to)` pair, so a validator needs
//!   exactly this list. Direction matters because those calls are directional: FPGA
//!   wires are one-way, so an edge cannot be assumed to work backwards.
//! - **Growth only appends.** Adding a path never reorders what is there, so the order
//!   is stable and the edges a path added are simply the last ones. A negotiated
//!   congestion engine can charge just those for their resources.
//!
//! An edge list stores two nodes per edge where a parent index stores one node and one
//! integer. That is the same size for `u32` nodes, and up to twice the size for bulky
//! nodes such as a position and a layer.
//!
//! The other cost is that looking up a node means scanning the route, so
//! [`add_path`](Route::add_path) takes time linear in the size of the tree. Growing a
//! tree already has that cost, since each search is seeded from all of its nodes, so
//! routes do not carry an index that they would have to store for their whole lifetime.
//!
//! # Determinism
//!
//! Checks use hash maps for lookups only and never iterate them. They scan edges in
//! order, so a route with several faults always reports the same one.
//!
//! [`RoutingSpace`]: crate::space::RoutingSpace
//! [`RoutingSpace::expand`]: crate::space::RoutingSpace::expand
//! [`RoutingSpace::resources`]: crate::space::RoutingSpace::resources

use core::fmt;
use core::hash::Hash;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;

use crate::cost::Cost;
use crate::id::NetId;

/// A node that connects a terminal, and what it costs to connect there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AccessPoint<N> {
    /// The node a route has to reach.
    pub node: N,
    /// The cost of connecting the terminal through this node.
    pub cost: Cost,
}

/// Something a net has to connect, reachable through any one of its access points.
///
/// The access points are equivalent, which covers FPGA pin equivalence, through-hole
/// pads reachable from any layer, and swappable pins. There is always at least one, and
/// they keep the order they were given in, so engines can break ties by it.
///
/// Listing a node twice is allowed and harmless: a route reaches both entries or neither.
///
/// # Examples
///
/// ```
/// use guide::cost::Cost;
/// use guide::net::{AccessPoint, EmptyTerminal, Terminal};
///
/// // A through-hole pad, slightly cheaper to reach on its first layer.
/// let pad = Terminal::new(vec![
///     AccessPoint { node: 10, cost: Cost::ZERO },
///     AccessPoint { node: 11, cost: Cost::ONE },
/// ])?;
/// assert_eq!(pad.access_points().len(), 2);
///
/// assert_eq!(Terminal::<u32>::new(vec![]), Err(EmptyTerminal));
/// # Ok::<(), EmptyTerminal>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Terminal<N> {
    access_points: Vec<AccessPoint<N>>,
}

impl<N> Terminal<N> {
    /// Creates a terminal, or fails if `access_points` is empty.
    pub fn new(access_points: Vec<AccessPoint<N>>) -> Result<Self, EmptyTerminal> {
        if access_points.is_empty() {
            Err(EmptyTerminal)
        } else {
            Ok(Terminal { access_points })
        }
    }

    /// Creates a terminal with a single access point at no cost.
    pub fn at(node: N) -> Self {
        Terminal {
            access_points: vec![AccessPoint {
                node,
                cost: Cost::ZERO,
            }],
        }
    }

    /// Returns the access points, which are never empty, in the order they were given.
    #[inline]
    pub fn access_points(&self) -> &[AccessPoint<N>] {
        &self.access_points
    }
}

/// The error for a [`Terminal`] with no access points, which could never be connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EmptyTerminal;

impl fmt::Display for EmptyTerminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("terminal has no access points")
    }
}

impl Error for EmptyTerminal {}

/// A set of terminals that have to end up connected.
///
/// Real netlists contain nets with one terminal, such as an unconnected output, so they
/// are representable. They have nothing to connect, so they are trivially satisfied:
/// [`needs_routing`](Net::needs_routing) is false and an empty [`Route`] is valid for
/// them. Nets with no terminals are treated the same way.
///
/// # Examples
///
/// ```
/// use guide::id::NetId;
/// use guide::net::{Net, Route, Terminal};
///
/// let net = Net { id: NetId(0), terminals: vec![Terminal::at(1), Terminal::at(2)] };
/// assert!(net.needs_routing());
///
/// let stub = Net { id: NetId(1), terminals: vec![Terminal::at(1)] };
/// assert!(!stub.needs_routing());
/// assert_eq!(Route::new().validate(&stub), Ok(()));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Net<N> {
    /// Identifies the net within its problem.
    pub id: NetId,
    /// The terminals to connect. Errors refer to them by their index here.
    pub terminals: Vec<Terminal<N>>,
}

impl<N> Net<N> {
    /// Returns whether the net has at least two terminals, and so needs a route.
    ///
    /// Two terminals can share an access point. Such a net still needs a route, but
    /// a single node is enough.
    #[inline]
    pub fn needs_routing(&self) -> bool {
        self.terminals.len() >= 2
    }
}

/// A directed connection between two nodes of a [`Route`], pointing away from its root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edge<N> {
    /// The node nearer the root.
    pub from: N,
    /// The node further from the root.
    pub to: N,
}

/// A tree of nodes connecting a net, as plain data that can be checked without a router.
///
/// See the [module documentation](self) for why it is a root and a list of edges. A
/// route is not guaranteed to be valid: [`from_edges`](Route::from_edges) accepts
/// anything so that faults can be reported, and [`validate`](Route::validate) has the
/// final word. Equality compares edge order as well as content.
///
/// # Examples
///
/// ```
/// use guide::id::NetId;
/// use guide::net::{Edge, Net, Route, Terminal};
///
/// let net = Net {
///     id: NetId(0),
///     terminals: vec![Terminal::at(1), Terminal::at(3), Terminal::at(4)],
/// };
///
/// // Grow a tree the way an engine does: one path per terminal.
/// let mut route = Route::new();
/// route.add_path(&[1, 2, 3])?;
/// route.add_path(&[2, 4])?;
///
/// assert_eq!(route.validate(&net), Ok(()));
/// assert_eq!(route.edges()[2], Edge { from: 2, to: 4 });
/// # Ok::<(), guide::net::PathError<u32>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Route<N> {
    // Kept apart from the edges because a tree of one node has none. Private so that
    // edges without a root cannot exist, leaving one less fault to report.
    root: Option<N>,
    edges: Vec<Edge<N>>,
}

impl<N> Route<N> {
    /// Creates an empty route, ready to grow with [`add_path`](Route::add_path).
    #[inline]
    pub const fn new() -> Self {
        Route {
            root: None,
            edges: Vec::new(),
        }
    }

    /// Creates a route of a single node, for terminals that share an access point.
    #[inline]
    pub const fn from_root(root: N) -> Self {
        Route {
            root: Some(root),
            edges: Vec::new(),
        }
    }

    /// Creates a route from existing data, without checking it.
    ///
    /// This is how results from elsewhere get to the validator, so it accepts edges that
    /// do not form a tree. Use [`validate`](Route::validate) to find out.
    #[inline]
    pub const fn from_edges(root: N, edges: Vec<Edge<N>>) -> Self {
        Route {
            root: Some(root),
            edges,
        }
    }

    /// Returns whether the route has no nodes at all.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Returns the edges in the order they were added, which never changes.
    ///
    /// A validator can check each against
    /// [`RoutingSpace::expand`](crate::space::RoutingSpace::expand) and total their
    /// [`RoutingSpace::resources`](crate::space::RoutingSpace::resources).
    #[inline]
    pub fn edges(&self) -> &[Edge<N>] {
        &self.edges
    }
}

impl<N: Copy> Route<N> {
    /// Returns the node every edge leads away from, or `None` if the route is empty.
    #[inline]
    pub const fn root(&self) -> Option<N> {
        self.root
    }

    /// Returns the root, then the end of each edge in order.
    ///
    /// On a valid route this is every node exactly once, with each node after its
    /// parent if the route was grown with [`add_path`](Route::add_path). An invalid
    /// route can repeat nodes, or omit those that only ever start an edge.
    pub fn nodes(&self) -> impl Iterator<Item = N> {
        self.root
            .into_iter()
            .chain(self.edges.iter().map(|edge| edge.to))
    }
}

impl<N: Copy + Eq + Hash> Route<N> {
    /// Adds a path found by a search to the tree, and returns how many edges that added.
    ///
    /// The new edges are the last ones in [`edges`](Route::edges). On an error the route
    /// is left unchanged.
    ///
    /// The path has to run away from the tree, so its first node must be on the tree
    /// unless the tree is empty, in which case that node becomes the root. A path
    /// arriving at the tree from outside is an error, because its last edge would need
    /// reversing, which a directed space may not allow.
    ///
    /// Only the part of the path after its *last* node on the tree is added. Anything
    /// before that is either already part of the tree or would close a loop in it, and
    /// dropping it never disconnects the far end of the path. So a detour that leaves
    /// the tree and comes back leaves no trace, and a path ending on the tree adds
    /// nothing, as its destination is already connected.
    ///
    /// This takes time linear in the sizes of the path and the tree. It assumes that the
    /// route is a tree, and will not turn one that is not into one.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::net::{Edge, PathError, Route};
    ///
    /// let mut route = Route::new();
    /// assert_eq!(route.add_path(&[1, 2, 3]), Ok(2));
    ///
    /// // Walks along the tree from 1 to 2, and only the step off it is new.
    /// assert_eq!(route.add_path(&[1, 2, 4]), Ok(1));
    /// assert_eq!(route.edges().last(), Some(&Edge { from: 2, to: 4 }));
    ///
    /// assert_eq!(route.add_path(&[9, 3]), Err(PathError::StartsOffTree { node: 9 }));
    /// ```
    pub fn add_path(&mut self, path: &[N]) -> Result<usize, PathError<N>> {
        let Some(&first) = path.first() else {
            return Err(PathError::Empty);
        };

        // The map is sized by the path rather than the tree, which is usually far larger.
        let mut positions = HashMap::with_capacity(path.len());
        for (position, &node) in path.iter().enumerate() {
            if positions.insert(node, position).is_some() {
                return Err(PathError::RepeatedNode { node });
            }
        }

        let graft = if self.is_empty() {
            0
        } else {
            let on_tree = self
                .nodes()
                .filter_map(|node| positions.get(&node).copied());
            // Position 0 is the path's first node, so `min` shows whether it is on the
            // tree and `max` is where the new part begins.
            let (min, max) = on_tree.fold((usize::MAX, 0), |(min, max), position| {
                (min.min(position), max.max(position))
            });
            if min != 0 {
                return Err(PathError::StartsOffTree { node: first });
            }
            max
        };

        self.root.get_or_insert(first);
        let new_nodes = &path[graft..];
        self.edges.extend(new_nodes.windows(2).map(|pair| Edge {
            from: pair[0],
            to: pair[1],
        }));
        Ok(new_nodes.len() - 1)
    }

    /// Returns whether `node` appears anywhere in the route, including as the start of
    /// an edge. This scans the route, so it takes time linear in its size.
    pub fn contains(&self, node: N) -> bool {
        self.root == Some(node)
            || self
                .edges
                .iter()
                .any(|edge| edge.from == node || edge.to == node)
    }

    /// Checks that the route is a single tree, directed away from its root.
    ///
    /// An empty route counts as valid, being the state that growth starts from. Nodes
    /// that serve no terminal are not an error here either: they only waste resources.
    ///
    /// # Errors
    ///
    /// Returns the first fault, looking for those visible in a single edge
    /// ([`Cycle`](RouteError::Cycle) by self-loop, [`DuplicateEdge`](RouteError::DuplicateEdge),
    /// [`MultipleParents`](RouteError::MultipleParents)) in edge order, and then for
    /// connectivity faults, also in edge order.
    pub fn validate_tree(&self) -> Result<(), RouteError<N>> {
        let Some(root) = self.root else {
            return Ok(());
        };

        let mut parents = HashMap::with_capacity(self.edges.len());
        for &Edge { from, to } in &self.edges {
            if from == to {
                return Err(RouteError::Cycle { node: to });
            }
            if let Some(first) = parents.insert(to, from) {
                return Err(if first == from {
                    RouteError::DuplicateEdge { from, to }
                } else {
                    RouteError::MultipleParents {
                        node: to,
                        first,
                        second: from,
                    }
                });
            }
        }

        // Every node now has at most one parent, so it is enough to walk up from each
        // edge. A walk ends at the root, at a parentless node outside the tree, or by
        // going around a loop. Each node remembers the last walk through it: meeting the
        // current walk means a loop, and meeting an earlier one means the root is known
        // to be reachable from there, since that walk would have failed otherwise. This
        // also keeps the whole check linear.
        let mut last_walk = HashMap::with_capacity(self.edges.len());
        for (walk, edge) in self.edges.iter().enumerate() {
            let mut node = edge.from;
            loop {
                match last_walk.insert(node, walk) {
                    Some(seen) if seen == walk => return Err(RouteError::Cycle { node }),
                    Some(_) => break,
                    None => {}
                }
                match parents.get(&node) {
                    Some(&parent) => node = parent,
                    None if node == root => break,
                    None => return Err(RouteError::Disconnected { node }),
                }
            }
        }
        Ok(())
    }

    /// Checks that the route is a valid tree for `net`.
    ///
    /// On top of [`validate_tree`](Route::validate_tree), it has to reach at least one
    /// access point of every terminal. An empty route satisfies a net that does not
    /// [need routing](Net::needs_routing), but a route that exists is held to the same
    /// standard on every net, since it is consuming resources.
    ///
    /// The root does not have to be an access point. That suits undirected spaces, where
    /// a tree can start at a Steiner point, but in a directed space it is up to the engine
    /// to root the tree at the net's driver: nets do not say which terminal that is.
    ///
    /// # Errors
    ///
    /// Returns the first fault that [`validate_tree`](Route::validate_tree) finds, or
    /// else [`MissingTerminal`](RouteError::MissingTerminal) for the first terminal that
    /// is not reached.
    ///
    /// # Examples
    ///
    /// ```
    /// use guide::cost::Cost;
    /// use guide::id::NetId;
    /// use guide::net::{AccessPoint, Net, Route, RouteError, Terminal};
    ///
    /// // Either of nodes 2 and 3 connects the second terminal.
    /// let either = Terminal::new(vec![
    ///     AccessPoint { node: 2, cost: Cost::ZERO },
    ///     AccessPoint { node: 3, cost: Cost::ZERO },
    /// ]).unwrap();
    /// let net = Net { id: NetId(0), terminals: vec![Terminal::at(1), either] };
    ///
    /// let mut route = Route::new();
    /// route.add_path(&[1, 3]).unwrap();
    /// assert_eq!(route.validate(&net), Ok(()));
    ///
    /// let mut route = Route::new();
    /// route.add_path(&[1, 4]).unwrap();
    /// assert_eq!(route.validate(&net), Err(RouteError::MissingTerminal { terminal: 1 }));
    /// ```
    pub fn validate(&self, net: &Net<N>) -> Result<(), RouteError<N>> {
        self.validate_tree()?;
        if self.is_empty() && !net.needs_routing() {
            return Ok(());
        }

        let nodes: HashSet<N> = self.nodes().collect();
        for (terminal, candidates) in net.terminals.iter().enumerate() {
            let reached = candidates
                .access_points()
                .iter()
                .any(|point| nodes.contains(&point.node));
            if !reached {
                return Err(RouteError::MissingTerminal { terminal });
            }
        }
        Ok(())
    }
}

impl<N> Default for Route<N> {
    #[inline]
    fn default() -> Self {
        Route::new()
    }
}

/// What is wrong with a [`Route`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RouteError<N> {
    /// Following edges from `node` leads back to `node`.
    Cycle {
        /// A node on the loop.
        node: N,
    },
    /// The same edge appears more than once.
    DuplicateEdge {
        /// The start of the edge.
        from: N,
        /// The end of the edge.
        to: N,
    },
    /// Two different edges arrive at a node. Ignoring direction this closes a loop or
    /// joins two trees, and either way it is not a tree any more.
    MultipleParents {
        /// The node with two parents.
        node: N,
        /// The start of the first edge arriving at `node`.
        first: N,
        /// The start of the second edge arriving at `node`.
        second: N,
    },
    /// A node cannot be reached from the root. It is in a separate piece of the route,
    /// or an edge along the way points the wrong way.
    Disconnected {
        /// The start of the piece that the root does not lead to.
        node: N,
    },
    /// No access point of a terminal is on the route.
    MissingTerminal {
        /// The terminal's index in [`Net::terminals`].
        terminal: usize,
    },
}

impl<N: fmt::Debug> fmt::Display for RouteError<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::Cycle { node } => write!(f, "route has a cycle through {node:?}"),
            RouteError::DuplicateEdge { from, to } => {
                write!(f, "route repeats the edge from {from:?} to {to:?}")
            }
            RouteError::MultipleParents {
                node,
                first,
                second,
            } => write!(
                f,
                "route arrives at {node:?} from both {first:?} and {second:?}"
            ),
            RouteError::Disconnected { node } => {
                write!(f, "route's root does not lead to {node:?}")
            }
            RouteError::MissingTerminal { terminal } => {
                write!(f, "route does not reach terminal {terminal}")
            }
        }
    }
}

impl<N: fmt::Debug> Error for RouteError<N> {}

/// Why a path could not be added with [`Route::add_path`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PathError<N> {
    /// The path has no nodes.
    Empty,
    /// A node appears twice in the path, which no shortest path does.
    RepeatedNode {
        /// The repeated node.
        node: N,
    },
    /// The path does not start on a tree that already has nodes.
    StartsOffTree {
        /// The path's first node.
        node: N,
    },
}

impl<N: fmt::Debug> fmt::Display for PathError<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::Empty => f.write_str("path has no nodes"),
            PathError::RepeatedNode { node } => write!(f, "path visits {node:?} twice"),
            PathError::StartsOffTree { node } => {
                write!(f, "path starts at {node:?}, which is not on the tree")
            }
        }
    }
}

impl<N: fmt::Debug> Error for PathError<N> {}

/// Why a net has no route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnroutedReason {
    /// No path connects the net's terminals, whatever else is routed.
    NoPath,
    /// The budget ran out before the net was routed. It might still be routable.
    BudgetExhausted,
    /// The caller stopped the engine before the net was routed.
    Cancelled,
}

impl fmt::Display for UnroutedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UnroutedReason::NoPath => "no path",
            UnroutedReason::BudgetExhausted => "budget exhausted",
            UnroutedReason::Cancelled => "cancelled",
        })
    }
}

/// What became of one net.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NetOutcome<N> {
    /// The net has a route. It may be empty, if the net does not
    /// [need routing](Net::needs_routing).
    Routed(Route<N>),
    /// The net has no route.
    Unrouted(UnroutedReason),
}

/// Counters describing how an engine run went.
///
/// These are counts, deliberately with no wall-clock time, so that a [`RouteSet`] from
/// one run compares equal to the same result from another. More will be added as engines
/// need them, so create this with [`Default`] and set the fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RouteStats {
    /// Passes the engine made over the nets.
    pub iterations: u64,
    /// Searches the engine ran.
    pub searches: u64,
    /// Nodes expanded across all searches.
    pub nodes_expanded: u64,
}

/// The result of routing: a route or a reason for each net, plus statistics.
///
/// A result can be partial, so failure never loses what succeeded. A net holds one
/// outcome at a time, which means it can never be both routed and unrouted. Iteration is
/// in ascending [`NetId`] order, whatever order the outcomes were recorded in.
///
/// A set only knows about the nets recorded in it. Engines should record every net they
/// were given, including an empty route for those that do not
/// [need routing](Net::needs_routing), so that a missing net means something went wrong.
///
/// # Examples
///
/// ```
/// use guide::id::NetId;
/// use guide::net::{Route, RouteSet, UnroutedReason};
///
/// let mut result = RouteSet::new();
/// result.set_unrouted(NetId(7), UnroutedReason::NoPath);
/// let mut route = Route::new();
/// route.add_path(&[1, 2]).unwrap();
/// result.set_route(NetId(3), route);
/// result.stats.iterations += 1;
///
/// assert_eq!(result.routes().count(), 1);
/// assert_eq!(result.route(NetId(3)).unwrap().edges().len(), 1);
/// assert_eq!(
///     result.unrouted().collect::<Vec<_>>(),
///     [(NetId(7), UnroutedReason::NoPath)],
/// );
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RouteSet<N> {
    // A map of outcomes rather than two collections, so that a net cannot be in both.
    // A BTreeMap iterates by key, which keeps the order independent of how nets finish.
    outcomes: BTreeMap<NetId, NetOutcome<N>>,
    /// Counters for the run that produced this result.
    pub stats: RouteStats,
}

impl<N> RouteSet<N> {
    /// Creates a result with no nets recorded yet.
    #[inline]
    pub const fn new() -> Self {
        RouteSet {
            outcomes: BTreeMap::new(),
            stats: RouteStats {
                iterations: 0,
                searches: 0,
                nodes_expanded: 0,
            },
        }
    }

    /// Records a route for `net`, returning the outcome it replaces, if any.
    pub fn set_route(&mut self, net: NetId, route: Route<N>) -> Option<NetOutcome<N>> {
        self.outcomes.insert(net, NetOutcome::Routed(route))
    }

    /// Records that `net` has no route, returning the outcome it replaces, if any.
    ///
    /// The old route comes back to the caller rather than being dropped, so an engine
    /// that fails to improve on a route can put it back.
    pub fn set_unrouted(&mut self, net: NetId, reason: UnroutedReason) -> Option<NetOutcome<N>> {
        self.outcomes.insert(net, NetOutcome::Unrouted(reason))
    }

    /// Returns what became of `net`, or `None` if nothing is recorded for it.
    pub fn outcome(&self, net: NetId) -> Option<&NetOutcome<N>> {
        self.outcomes.get(&net)
    }

    /// Returns the route of `net`, if it has one.
    pub fn route(&self, net: NetId) -> Option<&Route<N>> {
        match self.outcomes.get(&net) {
            Some(NetOutcome::Routed(route)) => Some(route),
            _ => None,
        }
    }

    /// Returns every recorded outcome, by ascending net.
    pub fn outcomes(&self) -> impl Iterator<Item = (NetId, &NetOutcome<N>)> {
        self.outcomes.iter().map(|(&net, outcome)| (net, outcome))
    }

    /// Returns the routed nets and their routes, by ascending net.
    pub fn routes(&self) -> impl Iterator<Item = (NetId, &Route<N>)> {
        self.outcomes().filter_map(|(net, outcome)| match outcome {
            NetOutcome::Routed(route) => Some((net, route)),
            NetOutcome::Unrouted(_) => None,
        })
    }

    /// Returns the unrouted nets and the reasons for them, by ascending net.
    pub fn unrouted(&self) -> impl Iterator<Item = (NetId, UnroutedReason)> {
        self.outcomes().filter_map(|(net, outcome)| match outcome {
            NetOutcome::Routed(_) => None,
            NetOutcome::Unrouted(reason) => Some((net, *reason)),
        })
    }
}

impl<N> Default for RouteSet<N> {
    #[inline]
    fn default() -> Self {
        RouteSet::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::sample::Index;

    fn edge(from: u32, to: u32) -> Edge<u32> {
        Edge { from, to }
    }

    fn route(root: u32, edges: &[(u32, u32)]) -> Route<u32> {
        let edges = edges.iter().map(|&(from, to)| edge(from, to)).collect();
        Route::from_edges(root, edges)
    }

    fn point(node: u32, cost: u32) -> AccessPoint<u32> {
        AccessPoint {
            node,
            cost: Cost::from(cost),
        }
    }

    /// A net with one single-access terminal at each of `nodes`.
    fn net(nodes: &[u32]) -> Net<u32> {
        Net {
            id: NetId(0),
            terminals: nodes.iter().map(|&node| Terminal::at(node)).collect(),
        }
    }

    // Terminals and nets.

    #[test]
    fn terminal_needs_an_access_point() {
        assert_eq!(Terminal::<u32>::new(vec![]), Err(EmptyTerminal));
        let terminal = Terminal::new(vec![point(1, 2), point(3, 4)]).unwrap();
        assert_eq!(terminal.access_points(), [point(1, 2), point(3, 4)]);
        assert_eq!(Terminal::at(5).access_points(), [point(5, 0)]);
    }

    #[test]
    fn nets_need_routing_from_two_terminals() {
        assert!(!net(&[]).needs_routing());
        assert!(!net(&[1]).needs_routing());
        assert!(net(&[1, 2]).needs_routing());
    }

    #[test]
    fn empty_route_satisfies_nets_with_nothing_to_connect() {
        assert_eq!(Route::new().validate(&net(&[])), Ok(()));
        assert_eq!(Route::new().validate(&net(&[1])), Ok(()));
    }

    #[test]
    fn empty_route_does_not_satisfy_a_net_that_needs_routing() {
        assert_eq!(
            Route::new().validate(&net(&[1, 2])),
            Err(RouteError::MissingTerminal { terminal: 0 })
        );
    }

    #[test]
    fn route_on_a_single_terminal_net_must_still_reach_it() {
        assert_eq!(Route::from_root(1).validate(&net(&[1])), Ok(()));
        assert_eq!(
            route(1, &[(1, 2)]).validate(&net(&[3])),
            Err(RouteError::MissingTerminal { terminal: 0 })
        );
    }

    #[test]
    fn terminals_sharing_a_node_are_connected_by_that_node_alone() {
        let shared = Net {
            id: NetId(0),
            terminals: vec![
                Terminal::new(vec![point(1, 0), point(5, 0)]).unwrap(),
                Terminal::new(vec![point(2, 0), point(5, 0)]).unwrap(),
            ],
        };
        assert!(shared.needs_routing());
        assert_eq!(Route::from_root(5).validate(&shared), Ok(()));
    }

    // Valid and invalid routes, checked by hand.

    #[test]
    fn accepts_trees() {
        assert_eq!(Route::<u32>::new().validate_tree(), Ok(()));
        assert_eq!(Route::from_root(1).validate_tree(), Ok(()));
        // 1 -> 2 -> 3, and 2 -> 4 -> 5, 4 -> 6.
        let tree = route(1, &[(1, 2), (2, 3), (2, 4), (4, 5), (4, 6)]);
        assert_eq!(tree.validate(&net(&[1, 3, 5, 6])), Ok(()));
    }

    #[test]
    fn accepts_edges_in_any_order() {
        let tree = route(1, &[(4, 5), (2, 4), (1, 2), (2, 3)]);
        assert_eq!(tree.validate_tree(), Ok(()));
    }

    #[test]
    fn stubs_are_not_faults() {
        assert_eq!(route(1, &[(1, 2), (1, 3)]).validate(&net(&[1, 2])), Ok(()));
    }

    #[test]
    fn root_need_not_be_an_access_point() {
        assert_eq!(route(1, &[(1, 2), (1, 3)]).validate(&net(&[2, 3])), Ok(()));
    }

    #[test]
    fn rejects_a_separate_piece() {
        assert_eq!(
            route(1, &[(1, 2), (3, 4)]).validate_tree(),
            Err(RouteError::Disconnected { node: 3 })
        );
    }

    #[test]
    fn rejects_edges_pointing_at_the_root() {
        // Connected if direction is ignored, but nothing leads away from the root.
        assert_eq!(
            route(1, &[(2, 1)]).validate_tree(),
            Err(RouteError::Disconnected { node: 2 })
        );
        assert_eq!(
            route(1, &[(1, 2), (3, 2)]).validate_tree(),
            Err(RouteError::MultipleParents {
                node: 2,
                first: 1,
                second: 3
            })
        );
    }

    #[test]
    fn rejects_cycles() {
        assert_eq!(
            route(1, &[(1, 1)]).validate_tree(),
            Err(RouteError::Cycle { node: 1 })
        );
        assert_eq!(
            route(1, &[(1, 2), (2, 2)]).validate_tree(),
            Err(RouteError::Cycle { node: 2 })
        );
        // Through the root.
        assert_eq!(
            route(1, &[(1, 2), (2, 3), (3, 1)]).validate_tree(),
            Err(RouteError::Cycle { node: 1 })
        );
        // Beside the tree, where each node still has only one parent.
        assert_eq!(
            route(1, &[(1, 2), (3, 4), (4, 3)]).validate_tree(),
            Err(RouteError::Cycle { node: 3 })
        );
        // An edge and its reverse.
        assert_eq!(
            route(1, &[(1, 2), (2, 1)]).validate_tree(),
            Err(RouteError::Cycle { node: 1 })
        );
    }

    #[test]
    fn rejects_converging_branches() {
        assert_eq!(
            route(1, &[(1, 2), (1, 3), (2, 4), (3, 4)]).validate_tree(),
            Err(RouteError::MultipleParents {
                node: 4,
                first: 2,
                second: 3
            })
        );
    }

    #[test]
    fn rejects_duplicate_edges() {
        assert_eq!(
            route(1, &[(1, 2), (2, 3), (1, 2)]).validate_tree(),
            Err(RouteError::DuplicateEdge { from: 1, to: 2 })
        );
    }

    #[test]
    fn reports_the_missing_terminal() {
        assert_eq!(
            route(1, &[(1, 2)]).validate(&net(&[1, 2, 3, 4])),
            Err(RouteError::MissingTerminal { terminal: 2 })
        );
    }

    #[test]
    fn tree_faults_come_before_missing_terminals() {
        assert_eq!(
            route(1, &[(1, 2), (1, 2)]).validate(&net(&[9, 9])),
            Err(RouteError::DuplicateEdge { from: 1, to: 2 })
        );
    }

    #[test]
    fn any_one_access_point_connects_a_terminal() {
        let equivalent = Net {
            id: NetId(0),
            terminals: vec![
                Terminal::at(1),
                Terminal::new(vec![point(7, 0), point(2, 5), point(8, 1)]).unwrap(),
            ],
        };
        assert_eq!(route(1, &[(1, 2)]).validate(&equivalent), Ok(()));
        assert_eq!(route(1, &[(1, 8)]).validate(&equivalent), Ok(()));
        // Reaching several is fine too.
        assert_eq!(route(1, &[(1, 7), (7, 8)]).validate(&equivalent), Ok(()));
        assert_eq!(
            route(1, &[(1, 3)]).validate(&equivalent),
            Err(RouteError::MissingTerminal { terminal: 1 })
        );
    }

    #[test]
    fn nodes_only_need_the_documented_bounds() {
        // Deliberately not Ord, as spaces need not be able to order their nodes.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        struct Position {
            x: i64,
            layer: u8,
        }
        let at = |x, layer| Position { x, layer };

        let target = Net {
            id: NetId(0),
            terminals: vec![Terminal::at(at(0, 0)), Terminal::at(at(1, 1))],
        };
        let mut tree = Route::new();
        tree.add_path(&[at(0, 0), at(0, 1), at(1, 1)]).unwrap();
        assert_eq!(tree.validate(&target), Ok(()));
    }

    #[test]
    fn errors_display_their_details() {
        let cases: [(RouteError<u32>, &str); 5] = [
            (RouteError::Cycle { node: 1 }, "cycle through 1"),
            (
                RouteError::DuplicateEdge { from: 1, to: 2 },
                "edge from 1 to 2",
            ),
            (
                RouteError::MultipleParents {
                    node: 1,
                    first: 2,
                    second: 3,
                },
                "at 1 from both 2 and 3",
            ),
            (RouteError::Disconnected { node: 1 }, "lead to 1"),
            (RouteError::MissingTerminal { terminal: 1 }, "terminal 1"),
        ];
        for (error, expected) in cases {
            assert!(error.to_string().contains(expected), "{error}");
        }
        assert!(
            PathError::RepeatedNode { node: 1 }
                .to_string()
                .contains("1 twice")
        );
        assert_eq!(UnroutedReason::NoPath.to_string(), "no path");
    }

    // Adding paths.

    #[test]
    fn first_path_sets_the_root() {
        let mut tree = Route::new();
        assert_eq!(tree.add_path(&[1, 2, 3]), Ok(2));
        assert_eq!(tree.root(), Some(1));
        assert_eq!(tree.edges(), [edge(1, 2), edge(2, 3)]);
    }

    #[test]
    fn single_node_path_starts_a_tree_or_does_nothing() {
        let mut tree = Route::new();
        assert_eq!(tree.add_path(&[1]), Ok(0));
        assert_eq!(tree, Route::from_root(1));
        assert_eq!(tree.add_path(&[1]), Ok(0));
        assert_eq!(tree, Route::from_root(1));
    }

    #[test]
    fn path_starting_on_the_tree_branches_from_there() {
        let mut tree = route(1, &[(1, 2), (2, 3)]);
        assert_eq!(tree.add_path(&[2, 4, 5]), Ok(2));
        assert_eq!(
            tree.edges(),
            [edge(1, 2), edge(2, 3), edge(2, 4), edge(4, 5)]
        );
        assert_eq!(tree.validate(&net(&[3, 5])), Ok(()));
    }

    #[test]
    fn path_following_the_tree_adds_only_what_is_new() {
        let mut tree = route(1, &[(1, 2), (2, 3)]);
        assert_eq!(tree.add_path(&[1, 2, 3, 4]), Ok(1));
        assert_eq!(tree.edges(), [edge(1, 2), edge(2, 3), edge(3, 4)]);
        assert_eq!(tree.validate_tree(), Ok(()));
    }

    #[test]
    fn path_rejoining_the_tree_drops_the_detour() {
        // Adding 1 -> 5 -> 3 would give node 3 a second parent.
        let mut tree = route(1, &[(1, 2), (2, 3)]);
        assert_eq!(tree.add_path(&[1, 5, 3, 4]), Ok(1));
        assert_eq!(tree.edges(), [edge(1, 2), edge(2, 3), edge(3, 4)]);
        assert!(!tree.contains(5));
        assert_eq!(tree.validate_tree(), Ok(()));
    }

    #[test]
    fn path_between_tree_nodes_adds_nothing() {
        // No duplicate of 1 -> 2, and no shortcut from 1 to 3.
        let mut tree = route(1, &[(1, 2), (2, 3)]);
        let before = tree.clone();
        assert_eq!(tree.add_path(&[1, 2]), Ok(0));
        assert_eq!(tree.add_path(&[1, 3]), Ok(0));
        assert_eq!(tree.add_path(&[3, 1]), Ok(0));
        assert_eq!(tree.add_path(&[1, 5, 3]), Ok(0));
        assert_eq!(tree, before);
    }

    #[test]
    fn rejected_paths_leave_the_route_alone() {
        let mut tree = route(1, &[(1, 2)]);
        let before = tree.clone();
        assert_eq!(tree.add_path(&[]), Err(PathError::Empty));
        assert_eq!(
            tree.add_path(&[8, 9]),
            Err(PathError::StartsOffTree { node: 8 })
        );
        // Touching the tree later on is not enough: edge 8 -> 2 would run towards it.
        assert_eq!(
            tree.add_path(&[8, 2, 9]),
            Err(PathError::StartsOffTree { node: 8 })
        );
        assert_eq!(
            tree.add_path(&[1, 8, 8]),
            Err(PathError::RepeatedNode { node: 8 })
        );
        assert_eq!(
            tree.add_path(&[2, 8, 2]),
            Err(PathError::RepeatedNode { node: 2 })
        );
        assert_eq!(tree, before);
    }

    #[test]
    fn rejected_paths_leave_an_empty_route_empty() {
        let mut tree = Route::new();
        assert_eq!(tree.add_path(&[]), Err(PathError::Empty));
        assert_eq!(
            tree.add_path(&[1, 2, 1]),
            Err(PathError::RepeatedNode { node: 1 })
        );
        assert!(tree.is_empty());
    }

    #[test]
    fn root_only_tree_can_be_grown() {
        let mut tree = Route::from_root(1);
        assert_eq!(tree.add_path(&[1, 2]), Ok(1));
        assert_eq!(tree.validate(&net(&[1, 2])), Ok(()));
    }

    // Order.

    #[test]
    fn edges_keep_their_order() {
        // Node values chosen so that no sorted or hashed order would match by accident.
        let edges = [edge(50, 3), edge(3, 90), edge(50, 1), edge(1, 70)];
        let mut tree = Route::from_edges(50, edges.to_vec());
        assert_eq!(tree.validate_tree(), Ok(()));
        assert_eq!(tree.edges(), edges);
        assert_eq!(tree.nodes().collect::<Vec<_>>(), [50, 3, 90, 1, 70]);

        tree.add_path(&[90, 2, 40]).unwrap();
        assert_eq!(tree.edges()[..4], edges);
        assert_eq!(tree.edges()[4..], [edge(90, 2), edge(2, 40)]);
    }

    #[test]
    fn with_several_faults_the_first_edge_decides() {
        for _ in 0..20 {
            // Hash maps are seeded per instance, so repeat to catch any dependence on one.
            let faulty = route(1, &[(1, 2), (7, 8), (5, 6), (6, 5), (9, 2), (1, 2)]);
            assert_eq!(
                faulty.validate_tree(),
                Err(RouteError::MultipleParents {
                    node: 2,
                    first: 1,
                    second: 9
                })
            );
            let faulty = route(1, &[(1, 2), (7, 8), (5, 6), (6, 5)]);
            assert_eq!(
                faulty.validate_tree(),
                Err(RouteError::Disconnected { node: 7 })
            );
        }
    }

    // Route sets.

    #[test]
    fn route_set_keeps_successes_alongside_failures() {
        let mut set = RouteSet::new();
        set.set_route(NetId(5), route(1, &[(1, 2)]));
        set.set_unrouted(NetId(9), UnroutedReason::BudgetExhausted);
        set.set_route(NetId(1), Route::new());
        set.set_unrouted(NetId(0), UnroutedReason::NoPath);

        let routed: Vec<_> = set.routes().map(|(net, _)| net).collect();
        assert_eq!(routed, [NetId(1), NetId(5)]);
        let unrouted: Vec<_> = set.unrouted().collect();
        assert_eq!(
            unrouted,
            [
                (NetId(0), UnroutedReason::NoPath),
                (NetId(9), UnroutedReason::BudgetExhausted)
            ]
        );
        let all: Vec<_> = set.outcomes().map(|(net, _)| net).collect();
        assert_eq!(all, [NetId(0), NetId(1), NetId(5), NetId(9)]);

        assert_eq!(set.route(NetId(5)), Some(&route(1, &[(1, 2)])));
        assert_eq!(set.route(NetId(0)), None);
        assert_eq!(
            set.outcome(NetId(0)),
            Some(&NetOutcome::Unrouted(UnroutedReason::NoPath))
        );
        assert_eq!(set.outcome(NetId(2)), None);
    }

    #[test]
    fn route_set_holds_one_outcome_per_net() {
        let mut set = RouteSet::new();
        let first = route(1, &[(1, 2)]);
        assert_eq!(set.set_route(NetId(0), first.clone()), None);

        // Ripping up a route hands it back rather than losing it.
        assert_eq!(
            set.set_unrouted(NetId(0), UnroutedReason::Cancelled),
            Some(NetOutcome::Routed(first.clone()))
        );
        assert_eq!(set.routes().count(), 0);
        assert_eq!(
            set.set_route(NetId(0), first),
            Some(NetOutcome::Unrouted(UnroutedReason::Cancelled))
        );
        assert_eq!(set.unrouted().count(), 0);
    }

    #[test]
    fn route_set_order_ignores_recording_order() {
        let mut forward = RouteSet::new();
        let mut backward = RouteSet::new();
        for id in 0..10 {
            forward.set_route(NetId(id), Route::from_root(id));
            backward.set_route(NetId(9 - id), Route::from_root(9 - id));
        }
        assert_eq!(forward, backward);
    }

    #[test]
    fn new_route_set_matches_default_stats() {
        let set = RouteSet::<u32>::new();
        assert_eq!(set.stats, RouteStats::default());
        assert_eq!(set, RouteSet::default());
    }

    // Properties.

    /// A random tree of `sizes` nodes, with shuffled labels and edge order.
    fn tree(sizes: core::ops::Range<usize>) -> impl Strategy<Value = Route<u32>> {
        sizes
            .prop_flat_map(|size| {
                let labels: Vec<u32> = (0..size as u32).collect();
                (Just(labels).prop_shuffle(), vec(any::<Index>(), size - 1))
            })
            .prop_flat_map(|(labels, parents)| {
                let edges: Vec<_> = parents
                    .iter()
                    .enumerate()
                    .map(|(i, parent)| edge(labels[parent.index(i + 1)], labels[i + 1]))
                    .collect();
                (Just(labels[0]), Just(edges).prop_shuffle())
            })
            .prop_map(|(root, edges)| Route::from_edges(root, edges))
    }

    /// A label that no generated tree uses.
    const OFF_TREE: u32 = 1_000_000;

    /// A net that needs every node of `tree`, so losing any of them is a fault. Every
    /// other terminal also offers an access point that is not on the tree.
    fn net_covering(tree: &Route<u32>) -> Net<u32> {
        let terminals = tree
            .nodes()
            .enumerate()
            .map(|(i, node)| {
                if i % 2 == 0 {
                    Terminal::at(node)
                } else {
                    Terminal::new(vec![point(OFF_TREE, 0), point(node, 1)]).unwrap()
                }
            })
            .collect();
        Net {
            id: NetId(0),
            terminals,
        }
    }

    proptest! {
        #[test]
        fn random_trees_validate(tree in tree(1..40)) {
            prop_assert_eq!(tree.validate(&net_covering(&tree)), Ok(()));
            prop_assert_eq!(tree.nodes().count(), tree.edges().len() + 1);
        }

        #[test]
        fn duplicating_an_edge_is_caught(tree in tree(2..40), which: Index, at: Index) {
            let mut edges = tree.edges().to_vec();
            let copy = *which.get(&edges);
            edges.insert(at.index(edges.len() + 1), copy);
            let corrupt = Route::from_edges(tree.root().unwrap(), edges);
            prop_assert_eq!(
                corrupt.validate_tree(),
                Err(RouteError::DuplicateEdge { from: copy.from, to: copy.to })
            );
        }

        #[test]
        fn removing_an_edge_is_caught(tree in tree(2..40), which: Index) {
            let target = net_covering(&tree);
            let mut edges = tree.edges().to_vec();
            edges.remove(which.index(edges.len()));
            let corrupt = Route::from_edges(tree.root().unwrap(), edges);
            prop_assert!(corrupt.validate(&target).is_err());
        }

        #[test]
        fn reversing_an_edge_is_caught(tree in tree(2..40), which: Index) {
            let mut edges = tree.edges().to_vec();
            let flipped = which.get_mut(&mut edges);
            *flipped = edge(flipped.to, flipped.from);
            let corrupt = Route::from_edges(tree.root().unwrap(), edges);
            prop_assert!(corrupt.validate_tree().is_err());
        }

        #[test]
        fn an_extra_edge_between_tree_nodes_is_caught(
            tree in tree(2..40),
            from: Index,
            to: Index,
            at: Index,
        ) {
            let nodes: Vec<u32> = tree.nodes().collect();
            let mut edges = tree.edges().to_vec();
            edges.insert(at.index(edges.len() + 1), edge(*from.get(&nodes), *to.get(&nodes)));
            let corrupt = Route::from_edges(tree.root().unwrap(), edges);
            prop_assert!(corrupt.validate_tree().is_err());
        }

        #[test]
        fn a_separate_piece_is_caught(tree in tree(1..40), at: Index) {
            let mut edges = tree.edges().to_vec();
            edges.insert(at.index(edges.len() + 1), edge(OFF_TREE, OFF_TREE + 1));
            let corrupt = Route::from_edges(tree.root().unwrap(), edges);
            prop_assert_eq!(
                corrupt.validate_tree(),
                Err(RouteError::Disconnected { node: OFF_TREE })
            );
        }

        #[test]
        fn an_unreached_terminal_is_caught(tree in tree(1..40), at: Index) {
            let mut target = net_covering(&tree);
            let terminal = at.index(target.terminals.len() + 1);
            target.terminals.insert(terminal, Terminal::at(OFF_TREE));
            prop_assert_eq!(
                tree.validate(&target),
                Err(RouteError::MissingTerminal { terminal })
            );
        }

        #[test]
        fn growing_keeps_a_valid_tree(
            paths in vec((any::<Index>(), vec(0..30u32, 0..12)), 1..20),
        ) {
            let mut grown = Route::from_root(100);
            for (start, rest) in paths {
                let on_tree: Vec<u32> = grown.nodes().collect();
                let mut path = vec![*start.get(&on_tree)];
                for node in rest {
                    if !path.contains(&node) {
                        path.push(node);
                    }
                }

                // The expected result, worked out differently from add_path.
                let last_on_tree = path.iter().rposition(|node| on_tree.contains(node)).unwrap();
                let before = grown.edges().to_vec();

                let added = grown.add_path(&path).unwrap();

                prop_assert_eq!(added, path.len() - 1 - last_on_tree);
                prop_assert_eq!(grown.validate_tree(), Ok(()));
                prop_assert!(grown.contains(*path.last().unwrap()));
                // Growth only appends, and only takes steps from the path.
                prop_assert_eq!(&grown.edges()[..before.len()], &before[..]);
                for new in &grown.edges()[before.len()..] {
                    prop_assert!(path.windows(2).any(|w| w == [new.from, new.to]));
                }
            }
        }
    }
}
