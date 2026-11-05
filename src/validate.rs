//! Checks routing results independently of the router.
//!
//! A bug shared by a router and its checker is invisible, so this module uses the problem
//! model and [`RoutingSpace`] and nothing else. It never asks how a result was found, and
//! it shares no code with the engines or with the search.
//!
//! [`check`] takes a space, the problem's nets and a [`RouteSet`], and returns a [`Report`]
//! with every [`Fault`] it finds and a [`Summary`]. It does not stop at the first fault,
//! and any input is acceptable, since it is handed broken results by design.
//!
//! # What makes a result legal
//!
//! - Each net in the problem appears once and has an outcome, and no outcome belongs to a
//!   net outside the problem.
//! - Each route passes [`Route::validate`]. That reports one error per route, so
//!   fixing it can reveal another.
//! - The root of a route for a net that [needs routing](Net::needs_routing) is an access
//!   point of the net's [driver](Net::driver). An empty route has no root to blame, and
//!   [`Route::validate`] already reports it as missing a terminal. Other nets have nothing
//!   to drive, so their routes can be rooted anywhere.
//! - Each terminal of a net in the problem has an access point that the space
//!   [contains](RoutingSpace::contains), whether or not the net has a route. An access point
//!   elsewhere is no fault if another one exists, as that terminal can still be connected,
//!   and searches and engines do just that.
//! - Each node of a route is one that the space contains. That has to be checked by itself,
//!   as a route of a single node has no edge to be found wanting, and spaces report no
//!   resources for a node they do not have.
//! - Each edge is a step that [`RoutingSpace::expand`] offers that net.
//! - Each node of a route that has nodes [tied](RoutingSpace::non_configurable) to it, by
//!   steps that can't be switched off, has those nodes on the route as well. Whether the
//!   tying step is one of the route's edges doesn't matter, only that the node is there.
//! - No resource is used by more nets than its [capacity](RoutingSpace::capacity). Usage
//!   comes from [`RoutingSpace::resources`], for every edge and for the root. It counts
//!   distinct nets, so a net can use a resource along as many of its edges as it likes.
//!   Blocked and zero-capacity resources need no special treatment, as any use of them
//!   exceeds their capacity.
//!
//! An unrouted net is not a fault, because a partial result can be legal. So
//! [`Report::is_legal`] means there are no faults, and [`Report::is_complete`] means that
//! the result is legal and that every net has a route.
//!
//! [`check_fixed`] does all of this, and also checks that a result kept the routes that were
//! [fixed](crate::net::FixedRoute) for its nets.
//!
//! Three choices follow from wanting every fault that can be trusted:
//!
//! - Edges that are not steps still have their resources counted. A bogus edge claims its
//!   resources as much as a real one, and leaving them out could hide a conflict.
//! - A route for a net outside the problem is reported, and is otherwise not looked at.
//!   The space may have per-net rules, and has never heard of that net.
//! - When the problem lists a [`NetId`] twice, the route is checked against the definition
//!   listed first. The problem itself is at fault, so this only needs to be predictable.
//!
//! # Costs
//!
//! [`Report::cost`] gives each routed net's total: the step costs in edge order, then one
//! access cost per terminal in terminal order. Floating-point addition rounds differently
//! in another order, and this one is fixed. A step costs what
//! [`expand`](RoutingSpace::expand) says, before any congestion.
//!
//! A terminal can have several access points on the route, and it pays for one of them:
//!
//! - The driver pays for the access point at the route's root, which is the one in use.
//!   If a node is listed twice with different costs, the cheaper entry counts.
//! - Any other terminal pays for its cheapest access point on the route. The tree reaches
//!   them all, so it is free to connect through that one.
//! - A driver whose net has no need for routing can have a route rooted away from it, in
//!   which case it is treated like any other terminal.
//!
//! An empty route connects nothing and costs nothing, including its terminal's access.
//!
//! A cost is only given for a route with no fault of its own, as otherwise the sum would
//! leave out edges with no step cost or count others twice. Overuse does not affect the
//! cost, since that is a conflict between routes rather than a fault in one.
//!
//! # Order
//!
//! Faults come in a fixed order that does not depend on the order of `nets`, or on the
//! order that outcomes were recorded in:
//!
//! 1. [`DuplicateNet`](Fault::DuplicateNet), by ascending net.
//! 2. [`MissingOutcome`](Fault::MissingOutcome), by ascending net.
//! 3. [`UnknownNet`](Fault::UnknownNet), by ascending net.
//! 4. [`TerminalNotInSpace`](Fault::TerminalNotInSpace), by ascending net, then terminal.
//! 5. By ascending net, [`InvalidRoute`](Fault::InvalidRoute), then
//!    [`RootNotOnDriver`](Fault::RootNotOnDriver), then
//!    [`NodeNotInSpace`](Fault::NodeNotInSpace) in the order the route lists its nodes, then
//!    [`NotAStep`](Fault::NotAStep) in edge order, then
//!    [`NonConfigurableNotTaken`](Fault::NonConfigurableNotTaken) in the order the route
//!    lists its nodes and then the order the space lists what is tied to each.
//! 6. [`Overuse`](Fault::Overuse), by ascending resource, each listing nets in ascending
//!    order.
//! 7. [`FixedRouteNotKept`](Fault::FixedRouteNotKept), by ascending net, from
//!    [`check_fixed`] only.
//!
//! Hash maps are used for lookups and never iterated.
//!
//! # Examples
//!
//! ```
//! use guide::cost::Cost;
//! use guide::id::{NetId, NodeId};
//! use guide::net::{Net, Route, RouteSet, Terminal, UnroutedReason};
//! use guide::space::grid::Grid;
//! use guide::validate::{check, Fault};
//!
//! // Three cells in a row, with a net across them and another on the middle one.
//! let grid = Grid::new(3, 1, 1)?;
//! let nets = [
//!     Net { id: NetId(0), terminals: vec![Terminal::at(NodeId(0)), Terminal::at(NodeId(2))] },
//!     Net { id: NetId(1), terminals: vec![Terminal::at(NodeId(1))] },
//! ];
//!
//! let mut routes = RouteSet::new();
//! let mut route = Route::new();
//! route.add_path(&[NodeId(0), NodeId(1), NodeId(2)]).unwrap();
//! routes.set_route(NetId(0), route);
//! routes.set_unrouted(NetId(1), UnroutedReason::NoPath);
//!
//! // Legal as far as it goes, with one net still to do.
//! let report = check(&grid, &nets, &routes);
//! assert!(report.is_legal());
//! assert!(!report.is_complete());
//! assert_eq!(report.cost(NetId(0)), Some(Cost::from(2)));
//!
//! // Putting the other net on the middle cell overuses it.
//! routes.set_route(NetId(1), Route::from_root(NodeId(1)));
//! let report = check(&grid, &nets, &routes);
//! assert!(!report.is_legal());
//! assert_eq!(
//!     report.faults(),
//!     [Fault::Overuse {
//!         resource: NodeId(1),
//!         capacity: 1,
//!         nets: vec![NetId(0), NetId(1)],
//!     }],
//! );
//! # Ok::<(), guide::space::grid::GridError>(())
//! ```

use core::fmt;
use core::hash::Hash;
use std::collections::{HashMap, HashSet};
use std::error::Error;

use crate::cost::Cost;
use crate::id::NetId;
use crate::net::{Edge, FixedRoute, Net, NetOutcome, Route, RouteError, RouteSet, UnroutedReason};
use crate::space::{RoutingSpace, Step};

/// Something that makes a routing result illegal.
///
/// See the [module documentation](self) for the order that a [`Report`] lists these in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Fault<N, R> {
    /// The problem lists the same net more than once.
    DuplicateNet {
        /// The net.
        net: NetId,
        /// How many times the problem lists it.
        count: usize,
    },
    /// A net in the problem has no outcome at all, which is different from being unrouted.
    MissingOutcome {
        /// The net.
        net: NetId,
    },
    /// The result holds an outcome for a net that is not in the problem.
    UnknownNet {
        /// The net.
        net: NetId,
    },
    /// A route failed [`Route::validate`].
    InvalidRoute {
        /// The net.
        net: NetId,
        /// What [`Route::validate`] reported.
        error: RouteError<N>,
    },
    /// A route for a net that needs routing does not start at one of the access points of
    /// the net's driver.
    RootNotOnDriver {
        /// The net.
        net: NetId,
        /// The route's root.
        root: N,
    },
    /// No access point of a net's terminal is on a node that the space contains, so no route
    /// can connect it. The problem is at fault here, and an unrouted net does not excuse it.
    TerminalNotInSpace {
        /// The net.
        net: NetId,
        /// The terminal's index in [`Net::terminals`].
        terminal: usize,
    },
    /// A route uses a node that the space does not contain. Each distinct node is reported
    /// once per route, with the root and both ends of each edge all counted.
    NodeNotInSpace {
        /// The net.
        net: NetId,
        /// The node.
        node: N,
    },
    /// An edge of a route is not a step that the space offers the net.
    NotAStep {
        /// The net.
        net: NetId,
        /// The edge's position in [`Route::edges`].
        index: usize,
        /// The edge.
        edge: Edge<N>,
    },
    /// A route has a node, `from`, without a node, `to`, that is
    /// [tied](RoutingSpace::non_configurable) to it, so that the route can't be what it
    /// claims to be: using `from` uses `to`.
    NonConfigurableNotTaken {
        /// The net.
        net: NetId,
        /// The node on the route.
        from: N,
        /// The node tied to it, which is not on the route.
        to: N,
    },
    /// More nets use a resource than its capacity allows.
    Overuse {
        /// The resource.
        resource: R,
        /// How many nets it allows.
        capacity: u32,
        /// The nets using it, in ascending order.
        nets: Vec<NetId>,
    },
    /// A net's route does not keep the route that was fixed for it: it has another root, or
    /// leaves out one of the fixed edges. Only [`check_fixed`] reports this.
    FixedRouteNotKept {
        /// The net.
        net: NetId,
    },
}

impl<N: fmt::Debug, R: fmt::Debug> fmt::Display for Fault<N, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::DuplicateNet { net, count } => {
                write!(f, "net {} is listed {count} times", net.0)
            }
            Fault::MissingOutcome { net } => write!(f, "net {} has no outcome", net.0),
            Fault::UnknownNet { net } => {
                write!(f, "net {} has an outcome but is not in the problem", net.0)
            }
            Fault::InvalidRoute { net, error } => write!(f, "net {}: {error}", net.0),
            Fault::RootNotOnDriver { net, root } => write!(
                f,
                "net {}: route starts at {root:?}, which is not an access point of its driver",
                net.0
            ),
            Fault::TerminalNotInSpace { net, terminal } => write!(
                f,
                "net {}: terminal {terminal} has no access point in the space",
                net.0
            ),
            Fault::NodeNotInSpace { net, node } => {
                write!(
                    f,
                    "net {}: route uses {node:?}, which is not in the space",
                    net.0
                )
            }
            Fault::NotAStep { net, index, edge } => write!(
                f,
                "net {}: edge {index} from {:?} to {:?} is not a step of the space",
                net.0, edge.from, edge.to
            ),
            Fault::Overuse {
                resource,
                capacity,
                nets,
            } => {
                write!(
                    f,
                    "resource {resource:?} has capacity {capacity} but is used by nets"
                )?;
                for (position, net) in nets.iter().enumerate() {
                    let separator = if position == 0 { " " } else { ", " };
                    write!(f, "{separator}{}", net.0)?;
                }
                Ok(())
            }
            Fault::NonConfigurableNotTaken { net, from, to } => write!(
                f,
                "net {}: route uses {from:?} without {to:?}, which is tied to it",
                net.0
            ),
            Fault::FixedRouteNotKept { net } => {
                write!(f, "net {}: route does not keep its fixed route", net.0)
            }
        }
    }
}

impl<N: fmt::Debug, R: fmt::Debug> Error for Fault<N, R> {}

/// What the result says about one net of the problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NetStatus {
    /// The net has a route.
    Routed {
        /// The route's total as described in the [module documentation](self), or `None`
        /// if the route has a fault of its own, which leaves it without a meaningful one.
        cost: Option<Cost>,
    },
    /// The net is recorded as unrouted. This is not a fault.
    Unrouted(UnroutedReason),
    /// The result does not mention the net, which is reported as
    /// [`MissingOutcome`](Fault::MissingOutcome).
    Missing,
}

/// Figures describing a whole result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Summary {
    /// The distinct nets in the problem.
    pub nets: usize,
    /// How many of those have a route, legal or not.
    pub routed: usize,
    /// How many of those are recorded as unrouted.
    pub unrouted: usize,
    /// How many of those the result does not mention.
    pub missing: usize,
    /// The edges across all routes of nets in the problem.
    pub edges: usize,
    /// The distinct resources used by routes of nets in the problem.
    pub resources_used: usize,
    /// How many of those are used beyond their capacity.
    pub resources_overused: usize,
    /// The sum of the nets' costs by ascending net, leaving out routes without a cost.
    /// In a legal result, that leaves out nothing but unrouted nets.
    pub total_cost: Cost,
}

/// The verdict on a routing result, as returned by [`check`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Report<N, R> {
    faults: Vec<Fault<N, R>>,
    // Sorted by net, so lookups can search it.
    nets: Vec<(NetId, NetStatus)>,
    summary: Summary,
}

impl<N, R> Report<N, R> {
    /// Returns whether the result has no faults. It may still have unrouted nets.
    #[inline]
    pub fn is_legal(&self) -> bool {
        self.faults.is_empty()
    }

    /// Returns whether the result has no faults and no unrouted nets.
    #[inline]
    pub fn is_complete(&self) -> bool {
        self.is_legal() && self.summary.unrouted == 0
    }

    /// Returns every fault, in the order given in the [module documentation](self).
    #[inline]
    pub fn faults(&self) -> &[Fault<N, R>] {
        &self.faults
    }

    /// Returns the figures for the whole result.
    #[inline]
    pub const fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Returns each distinct net of the problem and its status, by ascending net.
    pub fn nets(&self) -> impl Iterator<Item = (NetId, NetStatus)> {
        self.nets.iter().copied()
    }

    /// Returns the status of `net`, or `None` if it is not in the problem.
    pub fn status(&self, net: NetId) -> Option<NetStatus> {
        self.nets
            .binary_search_by_key(&net, |&(id, _)| id)
            .ok()
            .and_then(|position| self.nets.get(position))
            .map(|&(_, status)| status)
    }

    /// Returns the total cost of the route of `net`, or `None` if it has no route or the
    /// route has a fault of its own.
    pub fn cost(&self, net: NetId) -> Option<Cost> {
        match self.status(net) {
            Some(NetStatus::Routed { cost }) => cost,
            _ => None,
        }
    }

    /// Returns the unrouted nets and the reasons for them, by ascending net.
    pub fn unrouted(&self) -> impl Iterator<Item = (NetId, UnroutedReason)> {
        self.nets.iter().filter_map(|&(net, status)| match status {
            NetStatus::Unrouted(reason) => Some((net, reason)),
            _ => None,
        })
    }
}

/// Buffers reused across nets, so that checking does not allocate per route.
struct Scratch<N, R> {
    /// The nodes of the current route that have been expanded.
    expanded: HashSet<N>,
    /// The steps out of those nodes. Holding them means a node with many edges on the
    /// route is expanded once and not scanned for each edge, which keeps the time linear.
    steps: HashMap<(N, N), Cost>,
    /// The output of one call to [`RoutingSpace::expand`].
    expansion: Vec<Step<N>>,
    /// The nodes of the current route.
    nodes: HashSet<N>,
    /// The resources of the current route.
    resources: Vec<R>,
    /// The nodes tied to one node of the current route.
    tied: Vec<N>,
}

/// Checks a routing result against its problem, and reports everything wrong with it.
///
/// See the [module documentation](self) for the rules, the cost of a route, and the order
/// of the faults. The time taken is close to linear in the size of the problem and the
/// routes, on top of the calls made to the space, and the result depends on the order of
/// `nets` only in which definition of a duplicate net is used.
#[must_use]
pub fn check<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    routes: &RouteSet<S::Node>,
) -> Report<S::Node, S::Resource> {
    // A stable sort, so that the definition listed first stays first among duplicates.
    let mut sorted: Vec<&Net<S::Node>> = nets.iter().collect();
    sorted.sort_by_key(|net| net.id);

    let mut faults = Vec::new();
    for group in sorted.chunk_by(|a, b| a.id == b.id) {
        if let [first, _, ..] = group {
            faults.push(Fault::DuplicateNet {
                net: first.id,
                count: group.len(),
            });
        }
    }
    sorted.dedup_by_key(|net| net.id);

    for net in &sorted {
        if routes.outcome(net.id).is_none() {
            faults.push(Fault::MissingOutcome { net: net.id });
        }
    }
    for (net, _) in routes.outcomes() {
        if sorted.binary_search_by_key(&net, |known| known.id).is_err() {
            faults.push(Fault::UnknownNet { net });
        }
    }

    for net in &sorted {
        for (terminal, access) in net.terminals.iter().enumerate() {
            let points = access.access_points();
            if !points.iter().any(|point| space.contains(point.node)) {
                faults.push(Fault::TerminalNotInSpace {
                    net: net.id,
                    terminal,
                });
            }
        }
    }

    let mut summary = Summary {
        nets: sorted.len(),
        ..Summary::default()
    };
    let mut statuses = Vec::with_capacity(sorted.len());
    let mut usage = Vec::new();
    let mut scratch = Scratch {
        expanded: HashSet::new(),
        steps: HashMap::new(),
        expansion: Vec::new(),
        nodes: HashSet::new(),
        resources: Vec::new(),
        tied: Vec::new(),
    };
    for net in &sorted {
        let status = match routes.outcome(net.id) {
            None => {
                summary.missing += 1;
                NetStatus::Missing
            }
            Some(NetOutcome::Unrouted(reason)) => {
                summary.unrouted += 1;
                NetStatus::Unrouted(*reason)
            }
            Some(NetOutcome::Routed(route)) => {
                summary.routed += 1;
                summary.edges += route.edges().len();
                let cost = check_route(space, net, route, &mut scratch, &mut faults);
                collect_usage(space, net.id, route, &mut scratch.resources, &mut usage);
                if let Some(cost) = cost {
                    summary.total_cost = summary.total_cost + cost;
                }
                NetStatus::Routed { cost }
            }
        };
        statuses.push((net.id, status));
    }

    // Each net contributed each resource once, so the length of a group is its net count,
    // and sorting by resource then net puts the groups and their nets in report order.
    usage.sort_unstable();
    for group in usage.chunk_by(|a, b| a.0 == b.0) {
        let Some(&(resource, _)) = group.first() else {
            continue;
        };
        summary.resources_used += 1;
        let capacity = space.capacity(resource);
        if group.len() as u64 > u64::from(capacity) {
            summary.resources_overused += 1;
            faults.push(Fault::Overuse {
                resource,
                capacity,
                nets: group.iter().map(|&(_, net)| net).collect(),
            });
        }
    }

    Report {
        faults,
        nets: statuses,
        summary,
    }
}

/// Checks a result as [`check`] does, and also that it kept the routes that were fixed for
/// its nets.
///
/// A [`FixedRoute`] is kept if the net's route has the same root and every one of its edges,
/// in any order, which allows for a net that was only partially pre-routed and grown from
/// there. A net with no route is not a fault, as for [`check`], and neither is a fixed route
/// for a net outside the problem, which [`check`] has nothing to say about either. Faults of
/// this kind come after all others, by ascending net.
///
/// # Examples
///
/// ```
/// use guide::id::{NetId, NodeId};
/// use guide::net::{FixedRoute, Net, Route, RouteSet, Terminal};
/// use guide::space::grid::Grid;
/// use guide::validate::{check_fixed, Fault};
///
/// let grid = Grid::new(3, 2, 1)?;
/// let net = Net { id: NetId(0), terminals: vec![Terminal::at(NodeId(0)), Terminal::at(NodeId(2))] };
/// let mut fixed = Route::new();
/// fixed.add_path(&[NodeId(0), NodeId(1), NodeId(2)]).unwrap();
///
/// // Another route between the same terminals is legal, but it is not the fixed one.
/// let mut other = Route::new();
/// other.add_path(&[NodeId(0), NodeId(3), NodeId(4), NodeId(5), NodeId(2)]).unwrap();
/// let mut routes = RouteSet::new();
/// routes.set_route(NetId(0), other);
///
/// let report = check_fixed(&grid, &[net], &routes, &[FixedRoute { net: NetId(0), route: fixed }]);
/// assert_eq!(report.faults(), [Fault::FixedRouteNotKept { net: NetId(0) }]);
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
#[must_use]
pub fn check_fixed<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    routes: &RouteSet<S::Node>,
    fixed: &[FixedRoute<S::Node>],
) -> Report<S::Node, S::Resource> {
    let mut report = check(space, nets, routes);
    let known: HashSet<NetId> = nets.iter().map(|net| net.id).collect();
    let mut not_kept: Vec<NetId> = fixed
        .iter()
        .filter(|fixed| known.contains(&fixed.net))
        .filter(|fixed| {
            routes
                .route(fixed.net)
                .is_some_and(|route| !keeps(route, &fixed.route))
        })
        .map(|fixed| fixed.net)
        .collect();
    not_kept.sort_unstable();
    not_kept.dedup();
    report.faults.extend(
        not_kept
            .into_iter()
            .map(|net| Fault::FixedRouteNotKept { net }),
    );
    report
}

/// Returns whether `route` has the root and every edge of `fixed`.
fn keeps<N: Copy + Eq + Hash>(route: &Route<N>, fixed: &Route<N>) -> bool {
    if fixed.is_empty() {
        return true;
    }
    if route.root() != fixed.root() {
        return false;
    }
    let edges: HashSet<Edge<N>> = route.edges().iter().copied().collect();
    fixed.edges().iter().all(|edge| edges.contains(edge))
}

/// Reports the faults of a single route, and returns its cost if it has none.
fn check_route<S: RoutingSpace>(
    space: &S,
    net: &Net<S::Node>,
    route: &Route<S::Node>,
    scratch: &mut Scratch<S::Node, S::Resource>,
    faults: &mut Vec<Fault<S::Node, S::Resource>>,
) -> Option<Cost> {
    let mut sound = true;

    if let Err(error) = route.validate(net) {
        faults.push(Fault::InvalidRoute { net: net.id, error });
        sound = false;
    }

    if net.needs_routing()
        && let (Some(root), Some(driver)) = (route.root(), net.driver())
        && !driver.access_points().iter().any(|p| p.node == root)
    {
        faults.push(Fault::RootNotOnDriver { net: net.id, root });
        sound = false;
    }

    // Not `Route::nodes`, because an invalid route can leave out a node that only starts edges.
    scratch.nodes.clear();
    let ends = route.edges().iter().flat_map(|edge| [edge.from, edge.to]);
    for node in route.root().into_iter().chain(ends) {
        if scratch.nodes.insert(node) && !space.contains(node) {
            faults.push(Fault::NodeNotInSpace { net: net.id, node });
            sound = false;
        }
    }

    scratch.expanded.clear();
    scratch.steps.clear();
    let mut total = Cost::ZERO;
    for (index, &edge) in route.edges().iter().enumerate() {
        // Spaces only have to expand what they contain, and a node they do not contain has
        // no steps to offer, which is what the edge gets reported for.
        if scratch.expanded.insert(edge.from) && space.contains(edge.from) {
            scratch.expansion.clear();
            space.expand(net.id, edge.from, &mut scratch.expansion);
            for step in &scratch.expansion {
                scratch
                    .steps
                    .entry((edge.from, step.to))
                    .or_insert(step.cost);
            }
        }
        match scratch.steps.get(&(edge.from, edge.to)) {
            Some(&cost) => total = total + cost,
            None => {
                faults.push(Fault::NotAStep {
                    net: net.id,
                    index,
                    edge,
                });
                sound = false;
            }
        }
    }

    // Asked of every space, as the validator takes no one's word for whether it has any.
    scratch.expanded.clear();
    let ends = route.edges().iter().flat_map(|edge| [edge.from, edge.to]);
    for node in route.root().into_iter().chain(ends) {
        if !scratch.expanded.insert(node) || !space.contains(node) {
            continue;
        }
        scratch.tied.clear();
        space.non_configurable(net.id, node, &mut scratch.tied);
        for &to in &scratch.tied {
            if !scratch.nodes.contains(&to) {
                faults.push(Fault::NonConfigurableNotTaken {
                    net: net.id,
                    from: node,
                    to,
                });
                sound = false;
            }
        }
    }

    if !sound {
        return None;
    }
    add_access_costs(net, route, total, &mut scratch.nodes)
}

/// Adds the access costs of a net's terminals to `total`, following the rule in the
/// [module documentation](self), or returns `None` if a terminal is not on the route.
///
/// A route that has passed [`Route::validate`] reaches every terminal, so `None` should
/// not happen, but there is no reason to panic if it does.
fn add_access_costs<N: Copy + Eq + Hash>(
    net: &Net<N>,
    route: &Route<N>,
    mut total: Cost,
    nodes: &mut HashSet<N>,
) -> Option<Cost> {
    // An empty route connects nothing, so it pays for no access.
    let Some(root) = route.root() else {
        return Some(total);
    };
    nodes.clear();
    nodes.extend(route.nodes());

    for (position, terminal) in net.terminals.iter().enumerate() {
        let points = terminal.access_points();
        let at_root = points.iter().filter(|p| p.node == root).map(|p| p.cost);
        let on_route = points
            .iter()
            .filter(|p| nodes.contains(&p.node))
            .map(|p| p.cost);
        let used = if position == 0 {
            at_root.min().or_else(|| on_route.min())
        } else {
            on_route.min()
        };
        total = total + used?;
    }
    Some(total)
}

/// Appends a `(resource, net)` pair for each distinct resource that a route uses.
///
/// This does not depend on the route being sound: the edges it holds are what it claims,
/// whether or not they are real.
fn collect_usage<S: RoutingSpace>(
    space: &S,
    net: NetId,
    route: &Route<S::Node>,
    resources: &mut Vec<S::Resource>,
    usage: &mut Vec<(S::Resource, NetId)>,
) {
    resources.clear();
    if let Some(root) = route.root() {
        space.resources(net, root, root, resources);
    }
    for edge in route.edges() {
        space.resources(net, edge.from, edge.to, resources);
    }
    resources.sort_unstable();
    resources.dedup();
    usage.extend(resources.iter().map(|&resource| (resource, net)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;
    use crate::net::{AccessPoint, Terminal};
    use crate::space::InternedSlots;
    use crate::space::graph::{Graph, GraphBuilder};
    use crate::space::grid::{Connectivity, Grid};
    use core::cell::Cell;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::sample::Index;

    fn at(node: u32) -> Terminal<NodeId> {
        Terminal::at(NodeId(node))
    }

    /// A terminal with `(node, cost)` access points.
    fn either(points: &[(u32, u32)]) -> Terminal<NodeId> {
        let points = points
            .iter()
            .map(|&(node, cost)| AccessPoint {
                node: NodeId(node),
                cost: Cost::from(cost),
            })
            .collect();
        Terminal::new(points).unwrap()
    }

    fn net(id: u32, terminals: Vec<Terminal<NodeId>>) -> Net<NodeId> {
        Net {
            id: NetId(id),
            terminals,
        }
    }

    fn edge(from: u32, to: u32) -> Edge<NodeId> {
        Edge {
            from: NodeId(from),
            to: NodeId(to),
        }
    }

    fn route(root: u32, edges: &[(u32, u32)]) -> Route<NodeId> {
        let edges = edges.iter().map(|&(from, to)| edge(from, to)).collect();
        Route::from_edges(NodeId(root), edges)
    }

    /// A route along consecutive nodes, built without any help from a search.
    fn path(nodes: &[u32]) -> Route<NodeId> {
        let edges: Vec<_> = nodes.windows(2).map(|w| (w[0], w[1])).collect();
        route(nodes[0], &edges)
    }

    fn set(outcomes: Vec<(u32, Route<NodeId>)>) -> RouteSet<NodeId> {
        let mut routes = RouteSet::new();
        for (id, route) in outcomes {
            routes.set_route(NetId(id), route);
        }
        routes
    }

    /// `length` cells in a row, numbered from 0, with steps costing 1.
    fn row(length: u32) -> Grid {
        Grid::new(length, 1, 1).unwrap()
    }

    /// A graph from `(from, to, cost)` edges.
    fn graph(nodes: u32, edges: &[(u32, u32, u32)]) -> Graph {
        let mut builder = GraphBuilder::new(nodes);
        for &(from, to, cost) in edges {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                .unwrap();
        }
        builder.build()
    }

    // Fixed routes.

    fn fixed(id: u32, route: Route<NodeId>) -> FixedRoute<NodeId> {
        FixedRoute {
            net: NetId(id),
            route,
        }
    }

    #[test]
    fn a_route_that_grew_from_its_fixed_part_keeps_it() {
        let space = row(5);
        let nets = [net(0, vec![at(0), at(2), at(4)])];
        // The fixed part comes after the rest here, as order does not matter.
        let routes = set(vec![(0, route(0, &[(2, 3), (3, 4), (0, 1), (1, 2)]))]);
        let report = check_fixed(&space, &nets, &routes, &[fixed(0, path(&[0, 1, 2]))]);
        assert!(report.is_complete());
        assert_eq!(report, check(&space, &nets, &routes));
    }

    #[test]
    fn a_route_without_a_fixed_edge_or_root_does_not_keep_it() {
        let space = Grid::new(3, 2, 1).unwrap();
        let nets = [
            net(0, vec![at(0), at(2)]),
            net(1, vec![either(&[(3, 0), (5, 0)]), at(4)]),
        ];
        let routes = set(vec![(0, path(&[0, 1, 2])), (1, path(&[5, 4]))]);
        let given = [fixed(1, path(&[3, 4])), fixed(0, path(&[0, 3, 4, 5, 2]))];
        let report = check_fixed(&space, &nets, &routes, &given);
        assert!(!report.is_legal());
        assert_eq!(
            report.faults(),
            [
                Fault::FixedRouteNotKept { net: NetId(0) },
                Fault::FixedRouteNotKept { net: NetId(1) },
            ]
        );
        assert_eq!(
            report.faults()[0].to_string(),
            "net 0: route does not keep its fixed route"
        );
    }

    #[test]
    fn unrouted_and_unknown_nets_and_empty_fixed_routes_are_no_fault() {
        let space = row(3);
        let nets = [net(0, vec![at(0), at(2)]), net(1, vec![at(1)])];
        let mut routes = set(vec![(1, Route::new())]);
        routes.set_unrouted(NetId(0), UnroutedReason::NoPath);
        let given = [
            fixed(0, path(&[0, 1, 2])),
            fixed(1, Route::new()),
            fixed(7, path(&[0, 1])),
        ];
        let report = check_fixed(&space, &nets, &routes, &given);
        assert!(report.is_legal());
        assert!(!report.is_complete());
    }

    #[test]
    fn fixed_faults_come_after_every_other_fault() {
        let space = row(3);
        let nets = [net(0, vec![at(0), at(2)]), net(1, vec![at(0), at(1)])];
        let routes = set(vec![(0, path(&[0, 1, 2])), (1, path(&[0, 1]))]);
        let report = check_fixed(&space, &nets, &routes, &[fixed(0, path(&[0]))]);
        assert!(matches!(report.faults()[0], Fault::Overuse { .. }));
        assert_eq!(report.faults().len(), 2);
        // A root on its own is kept by any route from it.
        let report = check_fixed(&space, &nets, &routes, &[fixed(1, path(&[1]))]);
        assert_eq!(
            report.faults().last(),
            Some(&Fault::FixedRouteNotKept { net: NetId(1) })
        );
    }

    #[test]
    fn a_legal_result_on_a_grid_is_complete() {
        let grid = Grid::new(3, 3, 2).unwrap();
        let (a, b, c, d) = (
            grid.node_at(0, 0, 0).unwrap(),
            grid.node_at(1, 0, 0).unwrap(),
            grid.node_at(1, 1, 0).unwrap(),
            grid.node_at(1, 1, 1).unwrap(),
        );
        let e = grid.node_at(2, 2, 0).unwrap();
        let nets = [
            net(0, vec![at(a.0), at(c.0), at(d.0)]),
            net(1, vec![at(e.0)]),
        ];
        let tree = route(a.0, &[(a.0, b.0), (b.0, c.0), (c.0, d.0)]);
        let routes = set(vec![(0, tree), (1, Route::new())]);

        let report = check(&grid, &nets, &routes);

        assert_eq!(report.faults(), []);
        assert!(report.is_legal());
        assert!(report.is_complete());
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(3)));
        assert_eq!(report.cost(NetId(1)), Some(Cost::ZERO));
        assert_eq!(
            *report.summary(),
            Summary {
                nets: 2,
                routed: 2,
                unrouted: 0,
                missing: 0,
                edges: 3,
                resources_used: 4,
                resources_overused: 0,
                total_cost: Cost::from(3),
            }
        );
    }

    #[test]
    fn a_legal_result_on_a_graph_counts_base_costs_and_access_costs() {
        let mut builder = GraphBuilder::new(4);
        builder
            .add_edge(NodeId(0), NodeId(1), Cost::from(2))
            .unwrap();
        builder
            .add_edge(NodeId(1), NodeId(2), Cost::from(3))
            .unwrap();
        builder
            .add_edge(NodeId(1), NodeId(3), Cost::from(4))
            .unwrap();
        builder.set_base_cost(NodeId(1), Cost::from(10)).unwrap();
        let graph = builder.build();
        let nets = [net(
            0,
            vec![
                either(&[(0, 100)]),
                either(&[(2, 200)]),
                either(&[(3, 300)]),
            ],
        )];
        let routes = set(vec![(0, route(0, &[(0, 1), (1, 2), (1, 3)]))]);

        let report = check(&graph, &nets, &routes);

        assert!(report.is_complete());
        assert_eq!(
            report.cost(NetId(0)),
            Some(Cost::from(2 + 10 + 3 + 4 + 100 + 200 + 300))
        );
    }

    #[test]
    fn merged_parallel_edges_cost_the_cheapest_one() {
        let graph = graph(2, &[(0, 1, 9), (0, 1, 4)]);
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(&graph, &nets, &set(vec![(0, path(&[0, 1]))]));
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(4)));
    }

    #[test]
    fn an_empty_problem_with_no_outcomes_is_complete() {
        let report = check(&row(2), &[], &RouteSet::new());
        assert!(report.is_complete());
        assert_eq!(*report.summary(), Summary::default());
    }

    #[test]
    fn a_net_without_an_outcome_is_a_fault() {
        let nets = [net(4, vec![at(0), at(1)]), net(2, vec![at(2)])];
        let report = check(&row(3), &nets, &RouteSet::new());
        assert_eq!(
            report.faults(),
            [
                Fault::MissingOutcome { net: NetId(2) },
                Fault::MissingOutcome { net: NetId(4) },
            ]
        );
        assert_eq!(report.status(NetId(2)), Some(NetStatus::Missing));
        assert_eq!(report.summary().missing, 2);
        assert!(!report.is_complete());
    }

    #[test]
    fn outcomes_for_unknown_nets_are_faults_however_they_ended() {
        let nets = [net(1, vec![at(0)])];
        let mut routes = set(vec![(1, Route::new()), (9, path(&[1, 2]))]);
        routes.set_unrouted(NetId(5), UnroutedReason::Cancelled);

        let report = check(&row(3), &nets, &routes);

        assert_eq!(
            report.faults(),
            [
                Fault::UnknownNet { net: NetId(5) },
                Fault::UnknownNet { net: NetId(9) },
            ]
        );
        assert_eq!(report.status(NetId(9)), None);
    }

    #[test]
    fn a_route_for_an_unknown_net_is_not_examined() {
        // Neither its bogus edge nor its claim on node 0 is an extra fault, or a figure.
        let nets = [net(1, vec![at(0)])];
        let routes = set(vec![
            (1, Route::from_root(NodeId(0))),
            (9, route(0, &[(0, 2)])),
        ]);
        let report = check(&row(3), &nets, &routes);
        assert_eq!(report.faults(), [Fault::UnknownNet { net: NetId(9) }]);
        assert_eq!(report.summary().edges, 0);
        assert_eq!(report.summary().resources_used, 1);
    }

    #[test]
    fn duplicate_nets_are_reported_once_each_and_the_first_definition_is_used() {
        // Only the definition listed first accepts this route.
        let nets = [
            net(3, vec![at(0), at(1)]),
            net(1, vec![at(2)]),
            net(3, vec![at(2), at(0)]),
            net(1, vec![at(2)]),
            net(3, vec![at(2)]),
        ];
        let routes = set(vec![(3, path(&[0, 1])), (1, Route::new())]);
        let report = check(&row(3), &nets, &routes);
        assert_eq!(
            report.faults(),
            [
                Fault::DuplicateNet {
                    net: NetId(1),
                    count: 2
                },
                Fault::DuplicateNet {
                    net: NetId(3),
                    count: 3
                },
            ]
        );
        assert_eq!(report.summary().nets, 2);
        assert_eq!(report.nets().count(), 2);
    }

    #[test]
    fn duplicate_nets_count_once_towards_capacity() {
        let nets = [net(3, vec![at(0)]), net(3, vec![at(0)])];
        let routes = set(vec![(3, Route::from_root(NodeId(0)))]);
        let report = check(&row(1), &nets, &routes);
        assert_eq!(
            report.faults(),
            [Fault::DuplicateNet {
                net: NetId(3),
                count: 2
            }]
        );
    }

    #[test]
    fn route_errors_are_reported_as_they_are() {
        let nets = [net(0, vec![at(0), at(3)])];
        let cases = [
            (
                route(0, &[(0, 1), (0, 1), (1, 2), (2, 3)]),
                RouteError::DuplicateEdge {
                    from: NodeId(0),
                    to: NodeId(1),
                },
            ),
            (
                route(0, &[(0, 1), (2, 1), (1, 3)]),
                RouteError::MultipleParents {
                    node: NodeId(1),
                    first: NodeId(0),
                    second: NodeId(2),
                },
            ),
            (
                route(0, &[(0, 1), (2, 3)]),
                RouteError::Disconnected { node: NodeId(2) },
            ),
            (
                route(0, &[(0, 3), (1, 2), (2, 1)]),
                RouteError::Cycle { node: NodeId(1) },
            ),
            (path(&[0, 1]), RouteError::MissingTerminal { terminal: 1 }),
        ];
        for (route, error) in cases {
            let report = check(&row(4), &nets, &set(vec![(0, route)]));
            assert!(
                report.faults().contains(&Fault::InvalidRoute {
                    net: NetId(0),
                    error
                }),
                "{error:?} not in {:?}",
                report.faults()
            );
            assert_eq!(report.cost(NetId(0)), None);
        }
    }

    #[test]
    fn a_self_loop_is_an_invalid_route_and_not_a_step() {
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(&row(2), &nets, &set(vec![(0, route(0, &[(0, 1), (1, 1)]))]));
        assert_eq!(
            report.faults(),
            [
                Fault::InvalidRoute {
                    net: NetId(0),
                    error: RouteError::Cycle { node: NodeId(1) },
                },
                Fault::NotAStep {
                    net: NetId(0),
                    index: 1,
                    edge: edge(1, 1),
                },
            ]
        );
    }

    #[test]
    fn an_empty_route_on_a_net_needing_routing_is_only_a_missing_terminal() {
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(&row(2), &nets, &set(vec![(0, Route::new())]));
        assert_eq!(
            report.faults(),
            [Fault::InvalidRoute {
                net: NetId(0),
                error: RouteError::MissingTerminal { terminal: 0 },
            }]
        );
    }

    #[test]
    fn a_root_on_another_terminal_is_a_fault_even_on_a_good_tree() {
        let nets = [net(0, vec![at(2), at(0)])];
        let report = check(&row(3), &nets, &set(vec![(0, path(&[0, 1, 2]))]));
        assert_eq!(
            report.faults(),
            [Fault::RootNotOnDriver {
                net: NetId(0),
                root: NodeId(0),
            }]
        );
        assert_eq!(report.cost(NetId(0)), None);
    }

    #[test]
    fn a_root_on_any_access_point_of_the_driver_is_fine() {
        let nets = [net(0, vec![either(&[(5, 0), (1, 0)]), at(2)])];
        let report = check(&row(6), &nets, &set(vec![(0, path(&[1, 2]))]));
        assert_eq!(report.faults(), []);
    }

    #[test]
    fn a_root_on_a_steiner_point_is_a_fault() {
        let nets = [net(0, vec![at(0), at(2)])];
        let tree = route(1, &[(1, 0), (1, 2)]);
        let report = check(&row(3), &nets, &set(vec![(0, tree)]));
        assert_eq!(
            report.faults(),
            [Fault::RootNotOnDriver {
                net: NetId(0),
                root: NodeId(1),
            }]
        );
    }

    #[test]
    fn nets_with_zero_or_one_terminal_can_have_an_empty_route() {
        let nets = [net(0, vec![]), net(1, vec![either(&[(0, 7)])])];
        let routes = set(vec![(0, Route::new()), (1, Route::new())]);
        let report = check(&row(1), &nets, &routes);
        assert!(report.is_complete());
        // Nothing is connected, so no access is paid for.
        assert_eq!(report.cost(NetId(0)), Some(Cost::ZERO));
        assert_eq!(report.cost(NetId(1)), Some(Cost::ZERO));
        assert_eq!(report.summary().resources_used, 0);
    }

    #[test]
    fn a_route_on_a_net_with_no_terminals_still_counts() {
        let nets = [net(0, vec![]), net(1, vec![at(1)])];
        let routes = set(vec![(0, path(&[0, 1])), (1, Route::from_root(NodeId(1)))]);
        let report = check(&row(2), &nets, &routes);
        assert_eq!(
            report.faults(),
            [Fault::Overuse {
                resource: NodeId(1),
                capacity: 1,
                nets: vec![NetId(0), NetId(1)],
            }]
        );
        assert_eq!(report.cost(NetId(0)), Some(Cost::ONE));
    }

    #[test]
    fn a_route_on_a_single_terminal_net_has_to_reach_it_but_not_from_it() {
        let nets = [net(0, vec![either(&[(2, 7)])])];

        // Rooted away from the terminal, which is fine with nothing to drive.
        let report = check(&row(3), &nets, &set(vec![(0, path(&[0, 1, 2]))]));
        assert_eq!(report.faults(), []);
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(2 + 7)));

        let report = check(&row(3), &nets, &set(vec![(0, path(&[0, 1]))]));
        assert_eq!(
            report.faults(),
            [Fault::InvalidRoute {
                net: NetId(0),
                error: RouteError::MissingTerminal { terminal: 0 },
            }]
        );
    }

    #[test]
    fn terminals_sharing_a_node_need_no_edges_and_each_pay_for_access() {
        let nets = [net(0, vec![either(&[(1, 3)]), either(&[(1, 4)])])];
        let report = check(&row(2), &nets, &set(vec![(0, Route::from_root(NodeId(1)))]));
        assert!(report.is_complete());
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(7)));
    }

    #[test]
    fn edges_that_are_not_steps_are_reported_in_edge_order() {
        let nets = [net(0, vec![at(0), at(4)])];
        let tree = route(0, &[(0, 2), (2, 3), (3, 4)]);
        let report = check(&row(5), &nets, &set(vec![(0, tree)]));
        assert_eq!(
            report.faults(),
            [Fault::NotAStep {
                net: NetId(0),
                index: 0,
                edge: edge(0, 2),
            }]
        );
        assert_eq!(report.cost(NetId(0)), None);

        let tree = route(0, &[(0, 3), (3, 2), (2, 4)]);
        let report = check(&row(5), &nets, &set(vec![(0, tree)]));
        assert_eq!(
            report.faults(),
            [
                Fault::NotAStep {
                    net: NetId(0),
                    index: 0,
                    edge: edge(0, 3),
                },
                Fault::NotAStep {
                    net: NetId(0),
                    index: 2,
                    edge: edge(2, 4),
                },
            ]
        );
    }

    #[test]
    fn a_one_way_edge_cannot_be_walked_backwards() {
        let graph = graph(2, &[(0, 1, 1)]);
        let nets = [net(0, vec![at(1), at(0)])];
        let report = check(&graph, &nets, &set(vec![(0, path(&[1, 0]))]));
        assert_eq!(
            report.faults(),
            [Fault::NotAStep {
                net: NetId(0),
                index: 0,
                edge: edge(1, 0),
            }]
        );
    }

    #[test]
    fn steps_into_or_out_of_a_blocked_cell_are_not_steps() {
        let mut grid = row(3);
        grid.set_blocked(1, 0, 0, true).unwrap();
        let nets = [net(0, vec![at(0), at(2)])];
        let report = check(&grid, &nets, &set(vec![(0, path(&[0, 1, 2]))]));
        assert_eq!(
            report.faults(),
            [
                Fault::NotAStep {
                    net: NetId(0),
                    index: 0,
                    edge: edge(0, 1),
                },
                Fault::NotAStep {
                    net: NetId(0),
                    index: 1,
                    edge: edge(1, 2),
                },
                Fault::Overuse {
                    resource: NodeId(1),
                    capacity: 0,
                    nets: vec![NetId(0)],
                },
            ]
        );
    }

    #[test]
    fn two_nets_on_a_node_are_overuse_and_keep_their_costs() {
        let nets = [net(7, vec![at(0), at(1)]), net(2, vec![at(2), at(1)])];
        let routes = set(vec![(7, path(&[0, 1])), (2, path(&[2, 1]))]);
        let report = check(&row(3), &nets, &routes);
        assert_eq!(
            report.faults(),
            [Fault::Overuse {
                resource: NodeId(1),
                capacity: 1,
                nets: vec![NetId(2), NetId(7)],
            }]
        );
        assert_eq!(report.cost(NetId(2)), Some(Cost::ONE));
        assert_eq!(report.cost(NetId(7)), Some(Cost::ONE));
        assert_eq!(report.summary().total_cost, Cost::from(2));
        assert_eq!(report.summary().resources_overused, 1);
    }

    #[test]
    fn roots_use_resources_too() {
        // One route starts on the node that the other passes through.
        let nets = [net(0, vec![at(0), at(1)]), net(1, vec![at(0), at(0)])];
        let routes = set(vec![(0, path(&[0, 1])), (1, Route::from_root(NodeId(0)))]);
        let report = check(&row(2), &nets, &routes);
        assert_eq!(
            report.faults(),
            [Fault::Overuse {
                resource: NodeId(0),
                capacity: 1,
                nets: vec![NetId(0), NetId(1)],
            }]
        );
    }

    #[test]
    fn nets_up_to_capacity_are_fine_and_beyond_it_are_all_listed() {
        let mut grid = row(1);
        grid.set_capacity(0, 0, 0, 2).unwrap();
        let on_it = |id| net(id, vec![at(0)]);
        let root = || Route::from_root(NodeId(0));

        let report = check(
            &grid,
            &[on_it(0), on_it(1)],
            &set(vec![(0, root()), (1, root())]),
        );
        assert_eq!(report.faults(), []);

        let report = check(
            &grid,
            &[on_it(0), on_it(1), on_it(2)],
            &set(vec![(0, root()), (1, root()), (2, root())]),
        );
        assert_eq!(
            report.faults(),
            [Fault::Overuse {
                resource: NodeId(0),
                capacity: 2,
                nets: vec![NetId(0), NetId(1), NetId(2)],
            }]
        );
    }

    #[test]
    fn a_zero_capacity_resource_is_overused_by_a_single_net() {
        let mut grid = row(2);
        grid.set_capacity(1, 0, 0, 0).unwrap();
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(&grid, &nets, &set(vec![(0, path(&[0, 1]))]));
        assert_eq!(
            report.faults(),
            [Fault::Overuse {
                resource: NodeId(1),
                capacity: 0,
                nets: vec![NetId(0)],
            }]
        );
    }

    #[test]
    fn a_route_sitting_on_a_blocked_cell_is_overuse_with_no_step_involved() {
        let mut grid = row(1);
        grid.set_blocked(0, 0, 0, true).unwrap();
        let nets = [net(0, vec![at(0)])];
        let report = check(&grid, &nets, &set(vec![(0, Route::from_root(NodeId(0)))]));
        assert_eq!(
            report.faults(),
            [Fault::Overuse {
                resource: NodeId(0),
                capacity: 0,
                nets: vec![NetId(0)],
            }]
        );
    }

    #[test]
    fn a_graph_node_with_zero_capacity_is_overuse() {
        let mut builder = GraphBuilder::new(2);
        builder.add_edge(NodeId(0), NodeId(1), Cost::ONE).unwrap();
        builder.set_capacity(NodeId(1), 0).unwrap();
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(&builder.build(), &nets, &set(vec![(0, path(&[0, 1]))]));
        assert_eq!(report.summary().resources_overused, 1);
    }

    #[test]
    fn a_bogus_edge_still_claims_its_resources() {
        let nets = [net(0, vec![at(0), at(2)]), net(1, vec![at(2)])];
        let routes = set(vec![
            (0, route(0, &[(0, 2)])),
            (1, Route::from_root(NodeId(2))),
        ]);
        let report = check(&row(3), &nets, &routes);
        assert_eq!(
            report.faults(),
            [
                Fault::NotAStep {
                    net: NetId(0),
                    index: 0,
                    edge: edge(0, 2),
                },
                Fault::Overuse {
                    resource: NodeId(2),
                    capacity: 1,
                    nets: vec![NetId(0), NetId(1)],
                },
            ]
        );
    }

    #[test]
    fn a_repeated_edge_does_not_make_a_net_overuse_alone() {
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(&row(2), &nets, &set(vec![(0, route(0, &[(0, 1), (0, 1)]))]));
        assert_eq!(
            report.faults(),
            [Fault::InvalidRoute {
                net: NetId(0),
                error: RouteError::DuplicateEdge {
                    from: NodeId(0),
                    to: NodeId(1),
                },
            }]
        );
    }

    #[test]
    fn foreign_nodes_do_not_panic_and_edges_to_them_are_caught() {
        let far = u32::MAX;
        let nets = [net(0, vec![at(far), at(far - 1)]), net(1, vec![at(far)])];
        let routes = set(vec![(0, path(&[far, far - 1])), (1, path(&[0, far]))]);
        for report in [
            check(&row(2), &nets, &routes),
            check(&graph(2, &[(0, 1, 1)]), &nets, &routes),
        ] {
            assert_eq!(
                report.faults(),
                [
                    Fault::TerminalNotInSpace {
                        net: NetId(0),
                        terminal: 0,
                    },
                    Fault::TerminalNotInSpace {
                        net: NetId(0),
                        terminal: 1,
                    },
                    Fault::TerminalNotInSpace {
                        net: NetId(1),
                        terminal: 0,
                    },
                    Fault::NodeNotInSpace {
                        net: NetId(0),
                        node: NodeId(far),
                    },
                    Fault::NodeNotInSpace {
                        net: NetId(0),
                        node: NodeId(far - 1),
                    },
                    Fault::NotAStep {
                        net: NetId(0),
                        index: 0,
                        edge: edge(far, far - 1),
                    },
                    Fault::NodeNotInSpace {
                        net: NetId(1),
                        node: NodeId(far),
                    },
                    Fault::NotAStep {
                        net: NetId(1),
                        index: 0,
                        edge: edge(0, far),
                    },
                ]
            );
        }
    }

    #[test]
    fn a_lone_foreign_root_on_foreign_terminals_is_caught() {
        let far = u32::MAX;
        let nets = [net(0, vec![at(far), at(far)])];
        let routes = set(vec![(0, Route::from_root(NodeId(far)))]);
        for report in [
            check(&row(2), &nets, &routes),
            check(&graph(2, &[(0, 1, 1)]), &nets, &routes),
        ] {
            assert_eq!(
                report.faults(),
                [
                    Fault::TerminalNotInSpace {
                        net: NetId(0),
                        terminal: 0,
                    },
                    Fault::TerminalNotInSpace {
                        net: NetId(0),
                        terminal: 1,
                    },
                    Fault::NodeNotInSpace {
                        net: NetId(0),
                        node: NodeId(far),
                    },
                ]
            );
            assert!(!report.is_legal());
            assert_eq!(report.cost(NetId(0)), None);
        }
    }

    #[test]
    fn a_foreign_root_is_caught_when_the_terminals_are_not_on_it() {
        // Nothing but the root is wrong here, on a net that has nothing to drive.
        let far = u32::MAX;
        let nets = [net(0, vec![])];
        let report = check(
            &row(2),
            &nets,
            &set(vec![(0, Route::from_root(NodeId(far)))]),
        );
        assert_eq!(
            report.faults(),
            [Fault::NodeNotInSpace {
                net: NetId(0),
                node: NodeId(far),
            }]
        );
    }

    #[test]
    fn a_node_is_reported_once_per_route_however_often_it_appears() {
        let far = u32::MAX;
        let nets = [net(0, vec![]), net(1, vec![])];
        let tree = route(far, &[(far, 0), (far, 1)]);
        let routes = set(vec![(0, tree.clone()), (1, tree)]);
        let report = check(&row(2), &nets, &routes);
        let on_foreign: Vec<_> = report
            .faults()
            .iter()
            .filter(|fault| matches!(fault, Fault::NodeNotInSpace { .. }))
            .collect();
        let expected = |id| Fault::NodeNotInSpace {
            net: NetId(id),
            node: NodeId(far),
        };
        assert_eq!(on_foreign, [&expected(0), &expected(1)]);
    }

    #[test]
    fn a_node_that_only_starts_an_edge_is_still_checked() {
        // `Route::nodes` would not list `far`, since no edge arrives there.
        let far = u32::MAX;
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(
            &row(2),
            &nets,
            &set(vec![(0, route(0, &[(0, 1), (far, 1)]))]),
        );
        assert!(report.faults().contains(&Fault::NodeNotInSpace {
            net: NetId(0),
            node: NodeId(far),
        }));
    }

    #[test]
    fn a_terminal_outside_the_space_is_a_fault_on_an_unrouted_net_too() {
        let far = u32::MAX;
        let nets = [net(4, vec![at(0), at(far)]), net(2, vec![at(far)])];
        let mut routes = RouteSet::new();
        routes.set_unrouted(NetId(4), UnroutedReason::NoPath);
        routes.set_route(NetId(2), Route::new());
        let report = check(&row(2), &nets, &routes);
        assert_eq!(
            report.faults(),
            [
                Fault::TerminalNotInSpace {
                    net: NetId(2),
                    terminal: 0,
                },
                Fault::TerminalNotInSpace {
                    net: NetId(4),
                    terminal: 1,
                },
            ]
        );
    }

    #[test]
    fn a_terminal_with_any_access_point_in_the_space_is_no_fault() {
        let far = u32::MAX;
        let nets = [net(
            0,
            vec![either(&[(far, 0), (0, 2)]), either(&[(1, 3), (far - 1, 0)])],
        )];
        let report = check(&row(2), &nets, &set(vec![(0, path(&[0, 1]))]));
        assert_eq!(report.faults(), []);
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(1 + 2 + 3)));
    }

    #[test]
    fn a_blocked_cell_is_in_the_space() {
        // It is overused and not missing, which keeps the two faults apart.
        let mut grid = row(1);
        grid.set_blocked(0, 0, 0, true).unwrap();
        let nets = [net(0, vec![at(0)])];
        let report = check(&grid, &nets, &set(vec![(0, Route::from_root(NodeId(0)))]));
        assert!(matches!(report.faults(), [Fault::Overuse { .. }]));
    }

    #[test]
    fn a_space_is_not_asked_to_expand_a_node_it_does_not_have() {
        struct Fussy(Grid);
        impl RoutingSpace for Fussy {
            type Node = NodeId;
            type Resource = NodeId;
            type Slots = <Grid as RoutingSpace>::Slots;
            type ResourceSlots = <Grid as RoutingSpace>::ResourceSlots;
            fn new_slots(&self) -> Self::Slots {
                self.0.new_slots()
            }
            fn new_resource_slots(&self) -> Self::ResourceSlots {
                self.0.new_resource_slots()
            }
            fn contains(&self, node: NodeId) -> bool {
                self.0.contains(node)
            }
            fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
                assert!(self.0.contains(from), "asked to expand {from:?}");
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

        let far = u32::MAX;
        let nets = [net(0, vec![at(far), at(0)])];
        let report = check(&Fussy(row(1)), &nets, &set(vec![(0, path(&[far, 0]))]));
        assert!(report.faults().contains(&Fault::NotAStep {
            net: NetId(0),
            index: 0,
            edge: edge(far, 0),
        }));
    }

    #[test]
    fn engine_results_on_problems_with_foreign_terminals_agree_with_the_engine() {
        use crate::engine::{EngineOptions, route as run_engine};

        let far = u32::MAX;
        let grid = row(4);
        // The first net is unrouted and at fault, and the second has a spare access point.
        let nets = [
            net(0, vec![at(far), at(1)]),
            net(1, vec![either(&[(far, 0), (2, 0)]), at(3)]),
        ];
        let outcome = run_engine(&grid, &nets, EngineOptions::default()).unwrap();
        assert!(!outcome.is_success());
        let report = check(&grid, &nets, &outcome.routes);
        assert_eq!(
            report.faults(),
            [Fault::TerminalNotInSpace {
                net: NetId(0),
                terminal: 0,
            }]
        );
        assert_eq!(
            report.status(NetId(0)),
            Some(NetStatus::Unrouted(UnroutedReason::NoPath))
        );

        // With the first net gone, the result is legal in the eyes of both.
        let outcome = run_engine(&grid, &nets[1..], EngineOptions::default()).unwrap();
        assert!(outcome.is_success());
        assert!(check(&grid, &nets[1..], &outcome.routes).is_complete());
    }

    #[test]
    fn unrouted_nets_are_summarized_and_are_not_faults() {
        let nets = [
            net(0, vec![at(0), at(1)]),
            net(1, vec![at(2), at(3)]),
            net(2, vec![at(4)]),
        ];
        let mut routes = set(vec![(0, path(&[0, 1]))]);
        routes.set_unrouted(NetId(2), UnroutedReason::Cancelled);
        routes.set_unrouted(NetId(1), UnroutedReason::NoPath);

        let report = check(&row(5), &nets, &routes);

        assert!(report.is_legal());
        assert!(!report.is_complete());
        assert_eq!(report.faults(), []);
        assert_eq!(
            report.unrouted().collect::<Vec<_>>(),
            [
                (NetId(1), UnroutedReason::NoPath),
                (NetId(2), UnroutedReason::Cancelled),
            ]
        );
        assert_eq!(report.summary().routed, 1);
        assert_eq!(report.summary().unrouted, 2);
        assert_eq!(report.cost(NetId(0)), Some(Cost::ONE));
        assert_eq!(report.cost(NetId(1)), None);
        assert_eq!(
            report.nets().map(|(net, _)| net).collect::<Vec<_>>(),
            [NetId(0), NetId(1), NetId(2)]
        );
    }

    #[test]
    fn a_partial_result_can_be_illegal_and_is_then_not_complete() {
        let nets = [net(0, vec![at(0), at(2)]), net(1, vec![at(2), at(3)])];
        let mut routes = set(vec![(0, route(0, &[(0, 2)]))]);
        routes.set_unrouted(NetId(1), UnroutedReason::BudgetExhausted);
        let report = check(&row(4), &nets, &routes);
        assert!(!report.is_legal());
        assert!(!report.is_complete());
        assert_eq!(report.faults().len(), 1);
    }

    #[test]
    fn illegal_with_nothing_unrouted_is_not_complete() {
        let nets = [net(0, vec![at(0), at(1)])];
        let report = check(&row(2), &nets, &RouteSet::new());
        assert_eq!(report.summary().unrouted, 0);
        assert!(!report.is_complete());
    }

    /// One of each kind of fault, with some nets having several.
    fn many_faults() -> (Grid, Vec<Net<NodeId>>, RouteSet<NodeId>) {
        let nets = vec![
            net(6, vec![at(9), at(8)]), // Listed twice, and rooted off an unreached driver.
            net(3, vec![at(0), at(4)]), // Rooted on the wrong terminal, with a bogus edge.
            net(6, vec![at(9), at(8)]),
            net(5, vec![at(6), at(7)]), // No outcome.
            net(1, vec![at(0), at(2)]), // Overlaps net 3 on node 0 and net 4 on node 2.
            net(4, vec![at(2), at(3)]), // Unreached terminal.
            net(2, vec![at(10)]),       // Unrouted, which is not a fault.
        ];
        let mut routes = set(vec![
            (6, Route::from_root(NodeId(8))),
            (3, route(4, &[(4, 0)])),
            (1, path(&[0, 1, 2])),
            (4, Route::from_root(NodeId(2))),
            (8, Route::new()), // Not in the problem.
        ]);
        routes.set_unrouted(NetId(2), UnroutedReason::NoPath);
        routes.set_unrouted(NetId(7), UnroutedReason::NoPath); // Not in the problem.
        (row(11), nets, routes)
    }

    #[test]
    fn every_simultaneous_fault_is_listed_in_the_documented_order() {
        let (grid, nets, routes) = many_faults();
        let report = check(&grid, &nets, &routes);
        assert_eq!(
            report.faults(),
            [
                Fault::DuplicateNet {
                    net: NetId(6),
                    count: 2,
                },
                Fault::MissingOutcome { net: NetId(5) },
                Fault::UnknownNet { net: NetId(7) },
                Fault::UnknownNet { net: NetId(8) },
                Fault::RootNotOnDriver {
                    net: NetId(3),
                    root: NodeId(4),
                },
                Fault::NotAStep {
                    net: NetId(3),
                    index: 0,
                    edge: edge(4, 0),
                },
                Fault::InvalidRoute {
                    net: NetId(4),
                    error: RouteError::MissingTerminal { terminal: 1 },
                },
                Fault::InvalidRoute {
                    net: NetId(6),
                    error: RouteError::MissingTerminal { terminal: 0 },
                },
                Fault::RootNotOnDriver {
                    net: NetId(6),
                    root: NodeId(8),
                },
                Fault::Overuse {
                    resource: NodeId(0),
                    capacity: 1,
                    nets: vec![NetId(1), NetId(3)],
                },
                Fault::Overuse {
                    resource: NodeId(2),
                    capacity: 1,
                    nets: vec![NetId(1), NetId(4)],
                },
            ]
        );
    }

    #[test]
    fn the_report_ignores_the_order_of_nets_and_of_outcomes() {
        let (grid, mut nets, routes) = many_faults();
        let expected = check(&grid, &nets, &routes);
        assert_eq!(check(&grid, &nets, &routes), expected);

        nets.reverse();
        assert_eq!(check(&grid, &nets, &routes), expected);

        // Recording outcomes in the other order makes no difference either.
        let mut outcomes: Vec<_> = routes.outcomes().collect();
        outcomes.reverse();
        let mut rerecorded = RouteSet::new();
        for (net, outcome) in outcomes {
            match outcome {
                NetOutcome::Routed(route) => rerecorded.set_route(net, route.clone()),
                NetOutcome::Unrouted(reason) => rerecorded.set_unrouted(net, *reason),
            };
        }
        assert_eq!(check(&grid, &nets, &rerecorded), expected);
    }

    #[test]
    fn overuse_is_sorted_by_resource_whatever_the_route_order() {
        // Net 0 reaches the high nodes first.
        let nets = [net(0, vec![at(5), at(0)]), net(1, vec![at(0), at(5)])];
        let down = route(5, &[(5, 4), (4, 3), (3, 2), (2, 1), (1, 0)]);
        let up = route(0, &[(4, 5), (3, 4), (2, 3), (1, 2), (0, 1)]);
        let report = check(&row(6), &nets, &set(vec![(0, down), (1, up)]));
        let resources: Vec<_> = report
            .faults()
            .iter()
            .map(|fault| match fault {
                Fault::Overuse { resource, nets, .. } => {
                    assert_eq!(nets, &[NetId(0), NetId(1)]);
                    resource.0
                }
                other => panic!("unexpected fault {other}"),
            })
            .collect();
        assert_eq!(resources, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_non_driver_terminal_pays_for_its_cheapest_access_point_on_the_route() {
        // The cheapest of all, on node 4, is not on the route.
        let nets = [net(
            0,
            vec![at(0), either(&[(2, 9), (1, 5), (3, 6), (4, 1)])],
        )];
        let report = check(&row(5), &nets, &set(vec![(0, path(&[0, 1, 2, 3]))]));
        assert_eq!(report.faults(), []);
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(3 + 5)));
    }

    #[test]
    fn the_driver_pays_for_the_access_point_at_the_root_even_with_a_cheaper_one_reached() {
        let nets = [net(0, vec![either(&[(1, 1), (0, 8)]), at(2)])];
        let report = check(&row(3), &nets, &set(vec![(0, path(&[0, 1, 2]))]));
        assert_eq!(report.faults(), []);
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(2 + 8)));
    }

    #[test]
    fn a_node_listed_twice_pays_its_cheaper_entry() {
        let nets = [net(
            0,
            vec![either(&[(0, 8), (0, 3)]), either(&[(1, 8), (1, 2)])],
        )];
        let report = check(&row(2), &nets, &set(vec![(0, path(&[0, 1]))]));
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(1 + 3 + 2)));
    }

    #[test]
    fn a_single_terminal_driver_rooted_on_an_access_point_pays_for_that_one() {
        let nets = [net(0, vec![either(&[(1, 1), (0, 8)])])];
        let report = check(&row(2), &nets, &set(vec![(0, path(&[0, 1]))]));
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(1 + 8)));
    }

    #[test]
    fn step_costs_are_added_in_edge_order() {
        // 1e16 is where adding 1 starts to round away, so the order shows in the result.
        let big = Cost::new(1e16).unwrap();
        let mut builder = GraphBuilder::new(4);
        builder.add_edge(NodeId(0), NodeId(1), big).unwrap();
        builder.add_edge(NodeId(1), NodeId(2), Cost::ONE).unwrap();
        builder.add_edge(NodeId(2), NodeId(3), Cost::ONE).unwrap();
        let graph = builder.build();
        let nets = [net(0, vec![at(0), at(3)])];

        let report = check(
            &graph,
            &nets,
            &set(vec![(0, route(0, &[(0, 1), (1, 2), (2, 3)]))]),
        );
        assert_eq!(report.cost(NetId(0)).unwrap().get(), 1e16);

        let report = check(
            &graph,
            &nets,
            &set(vec![(0, route(0, &[(2, 3), (1, 2), (0, 1)]))]),
        );
        assert_eq!(report.faults(), []);
        assert_eq!(report.cost(NetId(0)).unwrap().get(), 1e16 + 2.0);
    }

    #[test]
    fn costs_saturate_rather_than_failing() {
        let mut builder = GraphBuilder::new(3);
        builder.add_edge(NodeId(0), NodeId(1), Cost::MAX).unwrap();
        builder.add_edge(NodeId(1), NodeId(2), Cost::MAX).unwrap();
        let nets = [net(0, vec![at(0), at(2)])];
        let routes = set(vec![(0, path(&[0, 1, 2]))]);
        let report = check(&builder.build(), &nets, &routes);
        assert_eq!(report.cost(NetId(0)), Some(Cost::MAX));
        assert_eq!(report.summary().total_cost, Cost::MAX);
    }

    #[test]
    fn faults_read_well() {
        let shown = [
            (
                Fault::TerminalNotInSpace {
                    net: NetId(1),
                    terminal: 2,
                },
                "net 1: terminal 2 has no access point in the space",
            ),
            (
                Fault::NodeNotInSpace {
                    net: NetId(1),
                    node: NodeId(9),
                },
                "net 1: route uses NodeId(9), which is not in the space",
            ),
            (
                Fault::DuplicateNet {
                    net: NetId(1),
                    count: 2,
                },
                "net 1 is listed 2 times",
            ),
            (
                Fault::MissingOutcome { net: NetId(1) },
                "net 1 has no outcome",
            ),
            (
                Fault::UnknownNet { net: NetId(1) },
                "net 1 has an outcome but is not in the problem",
            ),
            (
                Fault::InvalidRoute {
                    net: NetId(1),
                    error: RouteError::MissingTerminal { terminal: 3 },
                },
                "net 1: route does not reach terminal 3",
            ),
            (
                Fault::RootNotOnDriver {
                    net: NetId(1),
                    root: NodeId(4),
                },
                "net 1: route starts at NodeId(4), which is not an access point of its driver",
            ),
            (
                Fault::NotAStep {
                    net: NetId(1),
                    index: 2,
                    edge: edge(4, 5),
                },
                "net 1: edge 2 from NodeId(4) to NodeId(5) is not a step of the space",
            ),
            (
                Fault::Overuse {
                    resource: NodeId(4),
                    capacity: 1,
                    nets: vec![NetId(1), NetId(2)],
                },
                "resource NodeId(4) has capacity 1 but is used by nets 1, 2",
            ),
        ];
        for (fault, text) in shown {
            let fault: Fault<NodeId, NodeId> = fault;
            assert_eq!(fault.to_string(), text);
        }
    }

    /// What nets compete for in [`Bus`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    enum BusResource {
        Wire(u32),
        Shared,
    }

    /// A space whose rules differ per net, and whose resources are not its nodes.
    ///
    /// Node 0 fans out to nodes 1 to 4, and the others step to the next number. Net 9 can
    /// take no steps. Every use of a node also consumes a single shared resource, reported
    /// twice to show that entries get merged.
    #[derive(Default)]
    struct Bus {
        expansions: Cell<usize>,
    }

    impl RoutingSpace for Bus {
        type Node = u32;
        type Resource = BusResource;
        type Slots = InternedSlots<u32>;
        type ResourceSlots = InternedSlots<BusResource>;

        fn new_slots(&self) -> InternedSlots<u32> {
            InternedSlots::new()
        }

        fn new_resource_slots(&self) -> Self::ResourceSlots {
            InternedSlots::new()
        }

        fn contains(&self, node: u32) -> bool {
            node < u32::MAX
        }

        fn expand(&self, net: NetId, from: u32, out: &mut Vec<Step<u32>>) {
            self.expansions.set(self.expansions.get() + 1);
            if net == NetId(9) {
                return;
            }
            let targets = if from == 0 { 1..5 } else { from + 1..from + 2 };
            out.extend(targets.map(|to| Step {
                to,
                cost: Cost::ONE,
            }));
        }

        fn lower_bound(&self, _net: NetId, _from: u32, _to: u32) -> Cost {
            Cost::ZERO
        }

        fn resources(&self, _net: NetId, _from: u32, to: u32, out: &mut Vec<BusResource>) {
            out.extend([
                BusResource::Shared,
                BusResource::Wire(to),
                BusResource::Shared,
            ]);
        }

        fn capacity(&self, _resource: BusResource) -> u32 {
            1
        }
    }

    #[test]
    fn a_net_using_a_resource_along_many_edges_counts_once() {
        let space = Bus::default();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![Terminal::at(0), Terminal::at(3)],
        }];
        let mut routes = RouteSet::new();
        let mut route = Route::new();
        route.add_path(&[0, 1, 2, 3]).unwrap();
        routes.set_route(NetId(0), route);

        let report = check(&space, &nets, &routes);

        assert_eq!(report.faults(), []);
        assert_eq!(report.summary().resources_used, 5);
    }

    #[test]
    fn nets_sharing_a_resource_only_show_up_on_that_resource() {
        let space = Bus::default();
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![Terminal::at(0), Terminal::at(1)],
            },
            Net {
                id: NetId(1),
                terminals: vec![Terminal::at(5), Terminal::at(6)],
            },
        ];
        let mut routes = RouteSet::new();
        routes.set_route(
            NetId(0),
            Route::from_edges(0, vec![Edge { from: 0, to: 1 }]),
        );
        routes.set_route(
            NetId(1),
            Route::from_edges(5, vec![Edge { from: 5, to: 6 }]),
        );

        let report = check(&space, &nets, &routes);

        assert_eq!(
            report.faults(),
            [Fault::Overuse {
                resource: BusResource::Shared,
                capacity: 1,
                nets: vec![NetId(0), NetId(1)],
            }]
        );
    }

    #[test]
    fn steps_are_checked_against_the_rules_of_the_net_that_took_them() {
        let space = Bus::default();
        let terminals = vec![Terminal::at(5), Terminal::at(6)];
        let nets = [Net {
            id: NetId(9),
            terminals,
        }];
        let mut routes = RouteSet::new();
        let edges = vec![Edge { from: 5, to: 6 }];
        routes.set_route(NetId(9), Route::from_edges(5, edges));

        assert_eq!(
            check(&space, &nets, &routes).faults(),
            [Fault::NotAStep {
                net: NetId(9),
                index: 0,
                edge: Edge { from: 5, to: 6 },
            }]
        );
    }

    #[test]
    fn a_node_with_many_edges_on_the_route_is_expanded_once() {
        let space = Bus::default();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![Terminal::at(0), Terminal::at(4)],
        }];
        let edges = (1..5).map(|to| Edge { from: 0, to }).collect();
        let mut routes = RouteSet::new();
        routes.set_route(NetId(0), Route::from_edges(0, edges));

        let report = check(&space, &nets, &routes);

        assert_eq!(report.faults(), []);
        assert_eq!(report.cost(NetId(0)), Some(Cost::from(4)));
        assert_eq!(space.expansions.get(), 1);
    }

    /// How to grow one net's tree, as indices that get resolved against whatever the
    /// space offers at the time.
    #[derive(Clone, Debug)]
    struct NetPlan {
        root: Index,
        growth: Vec<(Index, Index)>,
        extra_terminals: Vec<Index>,
        cost_tenths: Vec<u32>,
    }

    fn plans() -> impl Strategy<Value = Vec<NetPlan>> {
        let net = (
            any::<Index>(),
            vec(any::<(Index, Index)>(), 1..12),
            vec(any::<Index>(), 0..3),
            vec(0u32..100, 8),
        )
            .prop_map(|(root, growth, extra_terminals, cost_tenths)| NetPlan {
                root,
                growth,
                extra_terminals,
                cost_tenths,
            });
        vec(net, 1..6)
    }

    fn grids() -> impl Strategy<Value = Grid> {
        (
            2u32..8,
            1u32..8,
            1u32..3,
            any::<bool>(),
            vec(any::<(Index, Index, Index)>(), 0..4),
        )
            .prop_map(|(width, height, layers, diagonals, blocks)| {
                let connectivity = if diagonals {
                    Connectivity::Octilinear
                } else {
                    Connectivity::Orthogonal
                };
                let mut grid = Grid::builder(width, height, layers)
                    .connectivity(connectivity)
                    .build()
                    .unwrap();
                for (x, y, layer) in blocks {
                    let (x, y) = (x.index(width as usize), y.index(height as usize));
                    let layer = layer.index(layers as usize);
                    grid.set_blocked(x as u32, y as u32, layer as u32, true)
                        .unwrap();
                }
                grid
            })
    }

    /// Graphs with a ring through all nodes, so that every node has somewhere to go.
    fn graphs() -> impl Strategy<Value = Graph> {
        (2u32..24, vec(any::<(Index, Index, u32)>(), 0..40)).prop_map(|(nodes, edges)| {
            let mut builder = GraphBuilder::new(nodes);
            for node in 0..nodes {
                builder
                    .add_edge(NodeId(node), NodeId((node + 1) % nodes), Cost::ONE)
                    .unwrap();
            }
            for (from, to, cost) in edges {
                let from = NodeId(from.index(nodes as usize) as u32);
                let to = NodeId(to.index(nodes as usize) as u32);
                let cost = Cost::new(f64::from(cost % 50) * 0.1).unwrap();
                builder.add_edge(from, to, cost).unwrap();
            }
            builder.build()
        })
    }

    /// A problem and a legal solution for it, with what each route should cost.
    struct Solved {
        nets: Vec<Net<NodeId>>,
        routes: RouteSet<NodeId>,
        costs: Vec<(NetId, Cost)>,
    }

    /// Grows random trees by walking `expand`, keeping every net on nodes of its own.
    ///
    /// All terminals go on the tree, and every leaf gets one, so that removing any edge
    /// from a route is sure to break it. Costs are added up here as the walk takes them.
    fn solve<S>(space: &S, node_count: u32, plans: &[NetPlan]) -> Solved
    where
        S: RoutingSpace<Node = NodeId>,
    {
        let mut taken = HashSet::new();
        let mut solved = Solved {
            nets: Vec::new(),
            routes: RouteSet::new(),
            costs: Vec::new(),
        };
        let mut offered = Vec::new();
        for (position, plan) in plans.iter().enumerate() {
            // Spaced out and descending, so that nothing relies on dense ascending IDs.
            let id = NetId(((plans.len() - position) * 3) as u32);
            let root = NodeId(plan.root.index(node_count as usize) as u32);
            if !is_free(space, &taken, root) {
                continue;
            }
            taken.insert(root);

            let mut tree = vec![root];
            let mut edges = Vec::new();
            let mut total = Cost::ZERO;
            for (from, step) in &plan.growth {
                let from = *from.get(&tree);
                offered.clear();
                space.expand(id, from, &mut offered);
                offered.retain(|step: &Step<NodeId>| is_free(space, &taken, step.to));
                if offered.is_empty() {
                    continue;
                }
                let step = *step.get(&offered);
                taken.insert(step.to);
                tree.push(step.to);
                edges.push(Edge { from, to: step.to });
                total = total + step.cost;
            }

            // The root, each leaf, then a few more. With no edges, the root twice.
            let mut nodes = vec![root];
            nodes.extend(
                tree.iter()
                    .copied()
                    .filter(|&node| node != root && !edges.iter().any(|e| e.from == node)),
            );
            nodes.extend(plan.extra_terminals.iter().map(|pick| *pick.get(&tree)));
            if nodes.len() < 2 {
                nodes.push(root);
            }

            let mut terminals = Vec::new();
            for (terminal, node) in nodes.into_iter().enumerate() {
                let tenths = plan.cost_tenths[terminal % plan.cost_tenths.len()];
                let cost = Cost::new(f64::from(tenths) * 0.1).unwrap();
                total = total + cost;
                terminals.push(Terminal::new(vec![AccessPoint { node, cost }]).unwrap());
            }

            solved.nets.push(Net { id, terminals });
            solved.routes.set_route(id, Route::from_edges(root, edges));
            solved.costs.push((id, total));
        }
        solved
    }

    fn is_free<S: RoutingSpace<Node = NodeId>>(
        space: &S,
        taken: &HashSet<NodeId>,
        node: NodeId,
    ) -> bool {
        if taken.contains(&node) {
            return false;
        }
        let mut resources = Vec::new();
        space.resources(NetId(0), node, node, &mut resources);
        resources
            .iter()
            .all(|&resource| space.capacity(resource) > 0)
    }

    /// Picks a route that has edges, returning its net and a copy.
    fn route_with_edges(
        solved: &Solved,
        pick: Index,
    ) -> Result<(NetId, Route<NodeId>), TestCaseError> {
        let candidates: Vec<_> = solved
            .routes
            .routes()
            .filter(|(_, route)| !route.edges().is_empty())
            .collect();
        if candidates.is_empty() {
            return Err(TestCaseError::reject("no route has edges"));
        }
        let (net, route) = *pick.get(&candidates);
        Ok((net, route.clone()))
    }

    fn solved_results_pass<S>(space: &S, count: u32, plans: &[NetPlan]) -> Result<(), TestCaseError>
    where
        S: RoutingSpace<Node = NodeId>,
    {
        let solved = solve(space, count, plans);
        let report = check(space, &solved.nets, &solved.routes);
        prop_assert!(report.faults().is_empty(), "{:?}", report.faults());
        prop_assert!(report.is_complete());
        for &(net, cost) in &solved.costs {
            prop_assert_eq!(report.cost(net), Some(cost));
        }

        let mut reversed = solved.nets.clone();
        reversed.reverse();
        prop_assert!(check(space, &reversed, &solved.routes) == report);
        Ok(())
    }

    fn a_dropped_edge_is_found<S>(
        space: &S,
        count: u32,
        plans: &[NetPlan],
        pick: Index,
        removed: Index,
    ) -> Result<(), TestCaseError>
    where
        S: RoutingSpace<Node = NodeId>,
    {
        let mut solved = solve(space, count, plans);
        let (net, route) = route_with_edges(&solved, pick)?;
        let mut edges = route.edges().to_vec();
        edges.remove(removed.index(edges.len()));
        let root = route.root().unwrap();
        solved.routes.set_route(net, Route::from_edges(root, edges));

        let report = check(space, &solved.nets, &solved.routes);

        prop_assert!(!report.is_legal());
        prop_assert!(report.cost(net).is_none());
        prop_assert!(
            report
                .faults()
                .iter()
                .any(|fault| matches!(fault, Fault::InvalidRoute { net: n, .. } if *n == net)),
            "{:?}",
            report.faults()
        );
        Ok(())
    }

    fn a_retargeted_edge_is_found<S>(
        space: &S,
        count: u32,
        plans: &[NetPlan],
        pick: Index,
        target: (Index, Index),
    ) -> Result<(), TestCaseError>
    where
        S: RoutingSpace<Node = NodeId>,
    {
        let mut solved = solve(space, count, plans);
        let (net, route) = route_with_edges(&solved, pick)?;
        let mut edges = route.edges().to_vec();
        let index = target.0.index(edges.len());
        let from = edges[index].from;

        // Search upwards from a random node. Past the last one is a foreign node, which no
        // step can reach, so this always ends.
        let mut offered = Vec::new();
        space.expand(net, from, &mut offered);
        let mut to = target.1.index(count as usize) as u32;
        while offered.iter().any(|step| step.to == NodeId(to)) {
            to += 1;
        }
        let bad = Edge {
            from,
            to: NodeId(to),
        };
        edges[index] = bad;
        let root = route.root().unwrap();
        solved.routes.set_route(net, Route::from_edges(root, edges));

        let report = check(space, &solved.nets, &solved.routes);

        prop_assert!(!report.is_legal());
        prop_assert!(report.cost(net).is_none());
        prop_assert!(
            report.faults().contains(&Fault::NotAStep {
                net,
                index,
                edge: bad
            }),
            "{:?}",
            report.faults()
        );
        Ok(())
    }

    fn an_overlapping_net_is_found<S>(
        space: &S,
        count: u32,
        plans: &[NetPlan],
        pick: Index,
        node: Index,
    ) -> Result<(), TestCaseError>
    where
        S: RoutingSpace<Node = NodeId, Resource = NodeId>,
    {
        let mut solved = solve(space, count, plans);
        let candidates: Vec<_> = solved.routes.routes().collect();
        prop_assume!(!candidates.is_empty());
        let (owner, route) = *pick.get(&candidates);
        let nodes: Vec<_> = route.nodes().collect();
        let node = *node.get(&nodes);

        // Higher than every other net, and legal apart from where it sits.
        let intruder = NetId((plans.len() as u32 + 1) * 3);
        solved.nets.push(Net {
            id: intruder,
            terminals: vec![Terminal::at(node), Terminal::at(node)],
        });
        solved.routes.set_route(intruder, Route::from_root(node));

        let report = check(space, &solved.nets, &solved.routes);

        let expected = [Fault::Overuse {
            resource: node,
            capacity: 1,
            nets: vec![owner, intruder],
        }];
        prop_assert_eq!(report.faults(), expected);
        prop_assert!(!report.is_legal());
        // Overuse is a conflict between routes, and leaves their costs meaningful.
        prop_assert!(report.cost(owner).is_some());
        Ok(())
    }

    proptest! {
        #[test]
        fn random_legal_routes_pass_on_a_grid(grid in grids(), plans in plans()) {
            solved_results_pass(&grid, grid.cell_count(), &plans)?;
        }

        #[test]
        fn random_legal_routes_pass_on_a_graph(graph in graphs(), plans in plans()) {
            solved_results_pass(&graph, graph.num_nodes(), &plans)?;
        }

        #[test]
        fn a_dropped_edge_is_found_on_a_grid(
            grid in grids(),
            plans in plans(),
            pick: Index,
            removed: Index,
        ) {
            a_dropped_edge_is_found(&grid, grid.cell_count(), &plans, pick, removed)?;
        }

        #[test]
        fn a_dropped_edge_is_found_on_a_graph(
            graph in graphs(),
            plans in plans(),
            pick: Index,
            removed: Index,
        ) {
            a_dropped_edge_is_found(&graph, graph.num_nodes(), &plans, pick, removed)?;
        }

        #[test]
        fn a_retargeted_edge_is_found_on_a_grid(
            grid in grids(),
            plans in plans(),
            pick: Index,
            target: (Index, Index),
        ) {
            a_retargeted_edge_is_found(&grid, grid.cell_count(), &plans, pick, target)?;
        }

        #[test]
        fn a_retargeted_edge_is_found_on_a_graph(
            graph in graphs(),
            plans in plans(),
            pick: Index,
            target: (Index, Index),
        ) {
            a_retargeted_edge_is_found(&graph, graph.num_nodes(), &plans, pick, target)?;
        }

        #[test]
        fn an_overlapping_net_is_found_on_a_grid(
            grid in grids(),
            plans in plans(),
            pick: Index,
            node: Index,
        ) {
            an_overlapping_net_is_found(&grid, grid.cell_count(), &plans, pick, node)?;
        }

        #[test]
        fn an_overlapping_net_is_found_on_a_graph(
            graph in graphs(),
            plans in plans(),
            pick: Index,
            node: Index,
        ) {
            an_overlapping_net_is_found(&graph, graph.num_nodes(), &plans, pick, node)?;
        }

        /// Whatever a result holds, checking it must not panic, and a route has a cost
        /// exactly when it has no fault of its own.
        #[test]
        fn arbitrary_results_do_not_panic(
            terminals in vec(vec(0u32..12, 0..4), 0..6),
            outcomes in vec((0u32..8, 0u32..12, vec((0u32..12, 0u32..12), 0..8)), 0..8),
        ) {
            let nets: Vec<_> = terminals
                .iter()
                .enumerate()
                .map(|(id, nodes)| net(id as u32 % 5, nodes.iter().map(|&n| at(n)).collect()))
                .collect();
            let mut routes = RouteSet::new();
            for (id, root, edges) in &outcomes {
                routes.set_route(NetId(*id), route(*root, edges));
            }
            let grid = row(10);
            let graph = graph(10, &[(0, 1, 1), (1, 2, 1), (2, 3, 1), (3, 0, 1)]);
            for report in [check(&grid, &nets, &routes), check(&graph, &nets, &routes)] {
                prop_assert_eq!(report.is_legal(), report.faults().is_empty());
                for fault in report.faults() {
                    prop_assert!(!fault.to_string().is_empty());
                }
                for (id, status) in report.nets() {
                    let has_own_fault = report.faults().iter().any(|fault| matches!(
                        fault,
                        Fault::InvalidRoute { net, .. }
                        | Fault::RootNotOnDriver { net, .. }
                        | Fault::NodeNotInSpace { net, .. }
                        | Fault::NotAStep { net, .. } if *net == id
                    ));
                    if let NetStatus::Routed { cost } = status {
                        prop_assert_eq!(cost.is_none(), has_own_fault);
                    }
                }
            }
        }
    }
}
