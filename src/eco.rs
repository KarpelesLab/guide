//! Engineering changes: routing again after a small change to a problem, keeping what did not
//! change, and measuring how much of a layout moved.
//!
//! An engineering change order (ECO) is a late, small change to a design: a net added or
//! removed, a pin moved, a block added, a resource taken away. What it should cost is a small
//! change to the layout, not a new one: what was verified, timed and reviewed should stay as it
//! was. The pieces are:
//!
//! - **Routing.** [`Engine::seed_routes`] starts each net from the route it had, and
//!   [`Engine::seed_history`] the negotiation from the history it had, so that only what changed
//!   or now conflicts is routed again. [`reroute`] and [`reroute_parallel`] do that from a
//!   [`RouteSet`]. The engine's documentation on [engineering
//!   changes](crate::engine#engineering-changes) says how.
//! - **Placement.** [`place::place_incremental`](crate::place::place_incremental) places new
//!   items and improves locally, with a [`Displacement`](crate::place::Displacement) term that
//!   charges for moving what was placed.
//! - **Measures.** [`route_change`] compares two route sets, and
//!   [`place::placement_change`](crate::place::placement_change) two placements.
//!
//! `docs/benchmarks.md` measures both against starting over, on generated problems and on VTR's
//! circuits.

use crate::cost::Cost;
use crate::engine::{Engine, EngineError, EngineOptions, Outcome, SeedReport};
use crate::id::NetId;
use crate::net::{Net, RouteSet};
use crate::space::{FastSet, RoutingSpace};

/// How much two route sets differ, net by net and node by node.
///
/// A net counts as **unchanged** when it is routed in both and its route has the same nodes,
/// whatever the order of its edges. Nodes are counted per net, so a node that two nets use
/// counts twice: `nodes_kept` is how many of the `(net, node)` pairs of the routes before are
/// still there after. On spaces whose resources are their nodes, as VPR's graphs and grids,
/// that is resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct RouteChange {
    /// Nets routed both before and after.
    pub nets_compared: u32,
    /// Of those, nets whose route has the same nodes.
    pub nets_unchanged: u32,
    /// Nets routed after but not before, as nets added.
    pub nets_added: u32,
    /// Nets routed before but not after, as nets removed.
    pub nets_removed: u32,
    /// `(net, node)` pairs of the routes before, over the nets compared.
    pub nodes_before: u64,
    /// `(net, node)` pairs of the routes after, over the nets compared.
    pub nodes_after: u64,
    /// `(net, node)` pairs in both, over the nets compared.
    pub nodes_kept: u64,
}

impl RouteChange {
    /// The fraction of the nets compared whose route did not change, 1 if none were compared.
    pub fn unchanged_fraction(&self) -> f64 {
        if self.nets_compared == 0 {
            1.0
        } else {
            f64::from(self.nets_unchanged) / f64::from(self.nets_compared)
        }
    }

    /// `(net, node)` pairs added and removed over the nets compared, as a fraction of those
    /// before: 0 for no change, and 2 for routes that share nothing with the same number of
    /// nodes.
    pub fn nodes_changed_fraction(&self) -> f64 {
        let changed = (self.nodes_before - self.nodes_kept) + (self.nodes_after - self.nodes_kept);
        if self.nodes_before == 0 {
            if changed == 0 { 0.0 } else { 1.0 }
        } else {
            changed as f64 / self.nodes_before as f64
        }
    }
}

/// Compares two route sets net by net: see [`RouteChange`].
///
/// # Examples
///
/// ```
/// use guide::eco::route_change;
/// use guide::id::NetId;
/// use guide::net::{Route, RouteSet};
///
/// let mut before = RouteSet::new();
/// let mut after = RouteSet::new();
/// let mut a = Route::new();
/// a.add_path(&[1, 2, 3]).unwrap();
/// let mut b = Route::new();
/// b.add_path(&[1, 4, 3]).unwrap();
/// before.set_route(NetId(0), a.clone());
/// after.set_route(NetId(0), a);
/// before.set_route(NetId(1), b.clone());
/// let mut moved = Route::new();
/// moved.add_path(&[1, 5, 3]).unwrap();
/// after.set_route(NetId(1), moved);
///
/// let change = route_change(&before, &after);
/// assert_eq!((change.nets_compared, change.nets_unchanged), (2, 1));
/// assert_eq!((change.nodes_before, change.nodes_kept), (6, 5));
/// ```
pub fn route_change<N: Copy + Eq + core::hash::Hash>(
    before: &RouteSet<N>,
    after: &RouteSet<N>,
) -> RouteChange {
    let mut change = RouteChange::default();
    for (net, old) in before.routes() {
        let Some(new) = after.route(net) else {
            change.nets_removed += 1;
            continue;
        };
        change.nets_compared += 1;
        // Only looked up and counted.
        let old_nodes: FastSet<N> = old.nodes().collect();
        let new_nodes: FastSet<N> = new.nodes().collect();
        let kept = new_nodes
            .iter()
            .filter(|node| old_nodes.contains(node))
            .count() as u64;
        change.nodes_before += old_nodes.len() as u64;
        change.nodes_after += new_nodes.len() as u64;
        change.nodes_kept += kept;
        if kept == old_nodes.len() as u64 && kept == new_nodes.len() as u64 {
            change.nets_unchanged += 1;
        }
    }
    change.nets_added = after
        .routes()
        .filter(|&(net, _)| before.route(net).is_none())
        .count() as u32;
    change
}

/// The present factor of the first iteration that [`stable_options`] sets.
pub const STABLE_FIRST_PRESENT: f64 = 10.0;

/// Returns `options` changed for routing again after an engineering change, so that routes that
/// were there move as little as they can:
///
/// - [`incremental_reroute`](EngineOptions::incremental_reroute) on, so that a net that has to
///   move keeps what of its route is not in a conflict;
/// - a [`first_present`](EngineOptions::first_present) of [`STABLE_FIRST_PRESENT`], 10, so that
///   in the first iteration, which only routes what changed, sharing what others hold costs
///   eleven times as much, and nets that changed go around those that did not, where they can,
///   rather than through them. The second iteration starts again from
///   [`initial_present`](EngineOptions::initial_present), as usual.
///
/// On the generated problems of `docs/benchmarks.md`, with 1% or 5% of the nets changed or
/// cells blocked, this kept 94% to 99.9% of the routes as they were, against 60% to 97% for
/// routing from scratch and 63% to 99.7% for seeds with the default options, at a cost within
/// half a percent of routing from scratch, in 0.1% to 4.3% of its expansions. With the default
/// options, nets that changed took resources from those that did not and set off a negotiation,
/// so that with 50 cells blocked, seeds kept fewer routes than routing from scratch did. A first
/// present factor of 3 kept fewer, and 1000 took up to two and a half times the expansions of
/// routing from scratch, as prices that high make a search's lower bound weak.
pub fn stable_options(options: EngineOptions<'_>) -> EngineOptions<'_> {
    EngineOptions {
        incremental_reroute: true,
        first_present: Cost::new(STABLE_FIRST_PRESENT).unwrap_or(options.first_present),
        ..options
    }
}

/// The result of routing again after an engineering change.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[must_use = "a result may hold conflicts, which only checking it will show"]
pub struct EcoOutcome<N, R> {
    /// The routing result, as from [`route`](crate::engine::route).
    pub outcome: Outcome<N, R>,
    /// What became of each route of the earlier result.
    pub seeds: SeedReport,
    /// How the routes differ from those of the earlier result.
    pub change: RouteChange,
    /// The history of the run, to seed the next with.
    pub history: Vec<(R, Cost)>,
}

/// Routes `nets` again after an engineering change, starting from `previous`, the routes of an
/// earlier result, and `history`, the history at its end if it was kept.
///
/// This is [`Engine::new`], [`Engine::seed_history`], [`Engine::seed_routes`] with every route
/// of `previous`, and [`Engine::run`]. See [engineering
/// changes](crate::engine#engineering-changes).
///
/// # Errors
///
/// Returns an error if two nets have the same ID.
///
/// # Examples
///
/// A grid with two routed nets, one of which is then blocked in the middle of its route.
/// Only that net moves.
///
/// ```
/// use guide::eco::reroute;
/// use guide::engine::{EngineOptions, route};
/// use guide::id::NetId;
/// use guide::net::{Net, Terminal};
/// use guide::space::grid::Grid;
///
/// let mut grid = Grid::new(6, 6, 1)?;
/// let at = |grid: &Grid, x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
/// let nets = [
///     Net { id: NetId(0), terminals: vec![at(&grid, 0, 1), at(&grid, 5, 1)] },
///     Net { id: NetId(1), terminals: vec![at(&grid, 0, 4), at(&grid, 5, 4)] },
/// ];
/// let before = route(&grid, &nets, EngineOptions::default()).unwrap();
///
/// grid.set_blocked(2, 1, 0, true)?;
/// let after = reroute(&grid, &nets, &before.routes, &[], EngineOptions::default()).unwrap();
///
/// assert!(after.outcome.is_success());
/// assert_eq!(after.change.nets_unchanged, 1);
/// assert_eq!(after.outcome.routes.route(NetId(1)), before.routes.route(NetId(1)));
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
pub fn reroute<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    previous: &RouteSet<S::Node>,
    history: &[(S::Resource, Cost)],
    options: EngineOptions<'_>,
) -> Result<EcoOutcome<S::Node, S::Resource>, EngineError> {
    let (mut engine, seeds) = seeded(space, nets, previous, history, options)?;
    while engine.step().is_some() {}
    Ok(finish(&engine, previous, seeds))
}

/// Routes as [`reroute`] does, with up to `threads` threads searching at once, and the same
/// result for every thread count. See [`route_parallel`](crate::engine::route_parallel).
///
/// # Errors
///
/// Returns an error if two nets have the same ID.
///
/// # Panics
///
/// A panic in a space's methods on any thread reaches the caller.
pub fn reroute_parallel<S>(
    space: &S,
    nets: &[Net<S::Node>],
    previous: &RouteSet<S::Node>,
    history: &[(S::Resource, Cost)],
    options: EngineOptions<'_>,
    threads: usize,
) -> Result<EcoOutcome<S::Node, S::Resource>, EngineError>
where
    S: RoutingSpace + Sync,
    S::Node: Send + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Send + Sync,
{
    let (mut engine, seeds) = seeded(space, nets, previous, history, options)?;
    while engine.step_parallel(threads).is_some() {}
    Ok(finish(&engine, previous, seeds))
}

/// Seeds an engine with every route of `previous` and with `history`.
fn seeded<'a, S: RoutingSpace>(
    space: &'a S,
    nets: &'a [Net<S::Node>],
    previous: &RouteSet<S::Node>,
    history: &[(S::Resource, Cost)],
    options: EngineOptions<'a>,
) -> Result<(Engine<'a, S>, SeedReport), EngineError> {
    let mut engine = Engine::new(space, nets, options)?;
    engine.seed_history(history.iter().copied())?;
    let seeds = previous
        .routes()
        .map(|(net, route): (NetId, _)| (net, route.clone()));
    let report = engine.seed_routes(seeds)?;
    Ok((engine, report))
}

/// The result of a seeded engine that has stopped.
fn finish<S: RoutingSpace>(
    engine: &Engine<'_, S>,
    previous: &RouteSet<S::Node>,
    seeds: SeedReport,
) -> EcoOutcome<S::Node, S::Resource> {
    let outcome = engine.outcome();
    let change = route_change(previous, &outcome.routes);
    EcoOutcome {
        outcome,
        seeds,
        change,
        history: engine.history(),
    }
}
