//! Starting a run from the routes and the history of an earlier one, after an engineering
//! change: see [engineering changes](super#engineering-changes).

use super::{Engine, EngineError, FixedPart, StepDelays};
use crate::cost::Cost;
use crate::id::NetId;
use crate::net::{Edge, Net, Route};
use crate::space::{FastMap, FastSet, RoutingSpace, Step};

/// What [`Engine::seed_routes`] made of the route a net was seeded with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Seeded {
    /// Taken whole: every edge is still a step of the space, and the route reaches every
    /// terminal. The net is routed again only if it holds an overused resource.
    Kept,
    /// Taken in part, and what is left reaches every terminal: the route went through nodes
    /// or steps that the space no longer has, or on to terminals that the net no longer has,
    /// and those branches were cut off. The net is routed again only if it holds an overused
    /// resource.
    Pruned,
    /// Taken in part, and what is left misses terminals, as when a terminal was added or
    /// moved: the net is routed in the first iteration, growing from what is left.
    Partial,
    /// Not taken: its root is no longer an access point of the driver, it is not a tree, or
    /// nothing of it is left. The net is routed from scratch.
    Dropped,
    /// Not taken: no net of the run has this ID, as when the net was removed.
    UnknownNet,
    /// Not taken: the net has a fixed route, needs no routing, can't be routed, or has a seed
    /// already.
    Ignored,
}

/// What [`Engine::seed_routes`] made of each route it was given.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SeedReport {
    /// Each net seeded, with what became of its route, in the order given.
    pub nets: Vec<(NetId, Seeded)>,
}

impl SeedReport {
    /// How many seeds came to `fate`.
    pub fn count(&self, fate: Seeded) -> usize {
        self.nets
            .iter()
            .filter(|&&(_, seeded)| seeded == fate)
            .count()
    }
}

impl<S: RoutingSpace> Engine<'_, S> {
    /// Starts each net from a route it had before, typically in an earlier run on a problem
    /// that has changed since. See [engineering changes](super#engineering-changes).
    ///
    /// Each route is checked against the net and the space as they are now. Branches that use
    /// a node or a step that the space no longer has, and branches that lead to no terminal,
    /// are cut off. If what is left reaches every terminal, the net has it as its route, which
    /// holds its resources from now on and is not routed again unless one of them is
    /// overused, so it stays as it was. If not, the net holds what is left until its turn in
    /// the first iteration, and then grows from it to the terminals it misses. Either way the
    /// route is a starting point and not a constraint: negotiation can move any part of it,
    /// as for any other route. [`fix_routes`](Engine::fix_routes) is for routes that must not
    /// move.
    ///
    /// Seeds for nets that the run does not have, or that are fixed, are ignored, and so is a
    /// second seed for a net. Call [`fix_routes`](Engine::fix_routes) first if both are
    /// needed: a net fixed after it was seeded loses its seed. In a timing-driven run, the
    /// delays along the seeds replace the estimates of their connections, and criticalities
    /// are worked out again.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::AlreadyStarted`] if an iteration has run, and then seeds
    /// nothing.
    ///
    /// # Examples
    ///
    /// A net added next to one routed before, which keeps its route, and the routes of the
    /// earlier run as they were.
    ///
    /// ```
    /// use guide::engine::{Engine, EngineOptions, Seeded, route};
    /// use guide::id::NetId;
    /// use guide::net::{Net, Terminal};
    /// use guide::space::grid::Grid;
    ///
    /// let grid = Grid::new(6, 6, 2)?;
    /// let at = |x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
    /// let mut nets = vec![Net { id: NetId(0), terminals: vec![at(0, 2), at(5, 2)] }];
    /// let before = route(&grid, &nets, EngineOptions::default()).unwrap();
    ///
    /// nets.push(Net { id: NetId(1), terminals: vec![at(0, 4), at(5, 4)] });
    /// let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
    /// let seeds = engine.seed_routes(
    ///     before.routes.routes().map(|(net, route)| (net, route.clone())),
    /// )?;
    /// assert_eq!(seeds.nets, [(NetId(0), Seeded::Kept)]);
    /// let after = engine.run();
    ///
    /// assert!(after.is_success());
    /// assert_eq!(after.routes.route(NetId(0)), before.routes.route(NetId(0)));
    /// assert_eq!(after.routes.stats.iterations, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn seed_routes(
        &mut self,
        routes: impl IntoIterator<Item = (NetId, Route<S::Node>)>,
    ) -> Result<SeedReport, EngineError> {
        if self.iteration > 0 {
            return Err(EngineError::AlreadyStarted);
        }
        // Looked up and never iterated.
        let index_of: FastMap<NetId, usize> = self
            .nets
            .iter()
            .enumerate()
            .map(|(index, net)| (net.id, index))
            .collect();
        let step_delays = self.timing.as_ref().map(|timing| timing.step_delays);
        let mut seeded = vec![false; self.nets.len()];
        let mut report = SeedReport::default();
        let mut any = false;
        for (id, route) in routes {
            let Some(&index) = index_of.get(&id) else {
                report.nets.push((id, Seeded::UnknownNet));
                continue;
            };
            let net = &self.nets[index];
            let state = &self.states[index];
            if seeded[index]
                || !net.needs_routing()
                || state.no_path
                || state.fixed.is_some()
                || state.route.is_some()
                || state.seed.is_some()
            {
                report.nets.push((id, Seeded::Ignored));
                continue;
            }
            seeded[index] = true;
            let Some((part, whole)) = trimmed_seed(self.space, step_delays, net, &route) else {
                report.nets.push((id, Seeded::Dropped));
                continue;
            };
            let fate = match (part.complete, whole) {
                (true, true) => Seeded::Kept,
                (true, false) => Seeded::Pruned,
                (false, _) => Seeded::Partial,
            };
            report.nets.push((id, fate));
            any = true;

            let slots: Vec<usize> = part
                .resources
                .iter()
                .map(|&resource| self.table.intern(self.space, resource))
                .collect();
            self.table.acquire(&slots);
            let state = &mut self.states[index];
            state.held = slots;
            state.resources.clone_from(&part.resources);
            if part.complete {
                state.route = Some(part.route);
                state.cost = part.cost;
                state.delays = part.delays;
            } else {
                state.seed = Some(Box::new(part));
            }
        }
        if any && let Some(timing) = &mut self.timing {
            timing.analyse(&self.states);
            timing.refresh_criticality();
        }
        Ok(report)
    }

    /// Sets the history of resources before the first iteration, typically to what
    /// [`history`](Engine::history) returned at the end of an earlier run on a similar
    /// problem, so that resources fought over then are dear from the start. See [engineering
    /// changes](super#engineering-changes).
    ///
    /// A resource listed twice gets the last of its values.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::AlreadyStarted`] if an iteration has run, and then sets
    /// nothing.
    pub fn seed_history(
        &mut self,
        history: impl IntoIterator<Item = (S::Resource, Cost)>,
    ) -> Result<(), EngineError> {
        if self.iteration > 0 {
            return Err(EngineError::AlreadyStarted);
        }
        for (resource, cost) in history {
            let slot = self.table.intern(self.space, resource);
            self.table.states[slot].history = cost;
        }
        Ok(())
    }

    /// Returns the history of every resource that has any, in ascending order of resource,
    /// for [`seed_history`](Engine::seed_history).
    pub fn history(&self) -> Vec<(S::Resource, Cost)> {
        let table = &self.table;
        let mut history: Vec<(S::Resource, Cost)> = table
            .occupied_slots
            .iter()
            .zip(&table.occupied_resources)
            .map(|(&slot, &resource)| (resource, table.states[slot].history))
            .filter(|&(_, cost)| cost != Cost::ZERO)
            .collect();
        history.sort_unstable_by_key(|&(resource, _)| resource);
        history
    }
}

/// What is left of a seed, and whether that is all of it.
type Trimmed<S> = (
    FixedPart<<S as RoutingSpace>::Node, <S as RoutingSpace>::Resource>,
    bool,
);

/// Returns what is left of a seed for `net` once every edge that is not a step of the space and
/// every branch that leads to no terminal is cut off, with whether nothing was cut, or `None` if
/// nothing can be kept.
fn trimmed_seed<S: RoutingSpace>(
    space: &S,
    step_delays: Option<StepDelays<S>>,
    net: &Net<S::Node>,
    route: &Route<S::Node>,
) -> Option<Trimmed<S>> {
    let root = route.root()?;
    let driver = net.driver()?;
    if !space.contains(root) || !driver.access_points().iter().any(|p| p.node == root) {
        return None;
    }
    route.validate_tree().ok()?;
    let edges = route.edges();

    // Down from the root, children in the order of the edges, so that the order is fixed
    // whatever order the edges came in.
    let mut children: FastMap<S::Node, Vec<usize>> = FastMap::default();
    for (index, edge) in edges.iter().enumerate() {
        children.entry(edge.from).or_default().push(index);
    }
    let mut order: Vec<usize> = Vec::with_capacity(edges.len());
    let mut keep = vec![false; edges.len()];
    let mut stack = vec![root];
    let mut steps: Vec<Step<S::Node>> = Vec::new();
    while let Some(node) = stack.pop() {
        let Some(below) = children.get(&node) else {
            continue;
        };
        steps.clear();
        space.expand(net.id, node, &mut steps);
        for &index in below.iter().rev() {
            let to = edges[index].to;
            if space.contains(to) && steps.iter().any(|step| step.to == to) {
                keep[index] = true;
                order.push(index);
                stack.push(to);
            }
        }
    }

    // Up from the leaves: an edge stays if it leads to a terminal or to an edge that stays.
    let mut needed: FastSet<S::Node> = FastSet::default();
    for terminal in net.terminals.iter().skip(1) {
        needed.extend(terminal.access_points().iter().map(|point| point.node));
    }
    let mut useful: FastSet<S::Node> = FastSet::default();
    for &index in order.iter().rev() {
        let Edge { from, to } = edges[index];
        if needed.contains(&to) || useful.contains(&to) {
            useful.insert(from);
        } else {
            keep[index] = false;
        }
    }
    let kept: Vec<Edge<S::Node>> = edges
        .iter()
        .zip(&keep)
        .filter(|&(_, &keep)| keep)
        .map(|(edge, _)| *edge)
        .collect();
    let whole = kept.len() == edges.len();
    if kept.is_empty() && !edges.is_empty() {
        return None;
    }
    let part = Engine::fixed_part(space, step_delays, net, Route::from_edges(root, kept)).ok()?;
    Some((part, whole))
}
