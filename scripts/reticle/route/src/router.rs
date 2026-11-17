//! The two routers on one placement: reticle's own, and guide's through [`Fabric`].
//!
//! Reticle's [`Routing`] has no public constructor that takes routes, and everything after
//! routing (verification, the bitstream, its decoding, timing) takes a `Routing`. So guide's
//! routes are handed to reticle by **replaying** them: reticle's own router is run again with
//! every node guide did not use closed (`RouteOptions::closed`), so that each signal can only
//! take the wires guide gave it. Whatever comes back is a `Routing` of reticle's making, held
//! to every one of reticle's rules, and [`compare`] says how many signals it gave exactly
//! guide's pips. What `reticle` would need to expose for this to be unnecessary is a way to
//! build a `Routing` from routes, such as `Routing::set(route)`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use guide::cost::Cost;
use guide::engine::{EngineOptions, Outcome, StopReason};
use guide::id::{NetId, NodeId};
use guide::net::{AccessPoint, Net, Terminal};
use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::place::{Netlist, Placement};
use reticle::fpga::route::{RouteOptions, Routing, route};

use crate::space::{Fabric, NetRules};

/// What one routing run took and gave.
#[derive(Clone, Debug, Default)]
pub struct RunReport {
    pub router: String,
    pub seconds: f64,
    /// Seconds to build what the router needs before it searches: guide's physical graph.
    pub setup_seconds: f64,
    pub iterations: u32,
    pub pips: usize,
    pub nodes: usize,
    pub signals: usize,
    /// Nodes taken off the queue (reticle) or expanded (guide).
    pub expansions: u64,
    /// The resident set before routing, and its peak during routing, in KiB.
    pub rss_before_kib: u64,
    pub rss_peak_kib: u64,
    /// For guide: how many signals the replay gave exactly guide's pips, and the replay's time.
    pub exact: Option<(usize, usize)>,
    pub replay_seconds: f64,
    pub replay_error: Option<String>,
    pub threads: usize,
}

/// A line of `/proc/self/status`, in KiB.
fn status_kib(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

/// Starts a measurement of peak memory from now: Linux resets the peak resident set when
/// `5` is written to `clear_refs`.
pub fn start_memory() -> u64 {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    status_kib("VmRSS:")
}

pub fn peak_memory() -> u64 {
    status_kib("VmHWM:")
}

/// Settings for guide.
#[derive(Clone, Debug)]
pub struct GuideConfig {
    pub threads: usize,
    pub batch_size: u32,
    /// These four are reticle's own for the family (`RouteOptions`) unless given.
    pub first_present: Option<f64>,
    pub initial_present: Option<f64>,
    pub present_growth: Option<f64>,
    pub history_factor: Option<f64>,
    pub incremental: bool,
    /// The cost of a tile in the lower bound, or `None` for reticle's (`astar_weight`).
    pub per_tile: Option<f64>,
    pub bound_weight: f64,
    /// `EngineOptions::target_terminals`.
    pub target_terminals: u32,
    pub max_iterations: u32,
    /// Whether to route by reticle's timing model, when the flow has one.
    pub timed: bool,
    /// What a unit of delay weighs against congestion, over reticle's own scale.
    pub delay_weight: f64,
    /// `EngineOptions::timing_iterations`.
    pub timing_iterations: u32,
    /// How the router was asked for, for the report.
    pub label: String,
}

impl Default for GuideConfig {
    fn default() -> Self {
        GuideConfig {
            threads: 1,
            batch_size: 1,
            first_present: None,
            initial_present: None,
            present_growth: None,
            history_factor: None,
            incremental: true,
            per_tile: None,
            bound_weight: 1.0,
            max_iterations: 100,
            target_terminals: 1,
            label: "guide".into(),
            timed: false,
            delay_weight: 1.0,
            timing_iterations: 0,
        }
    }
}

/// Reticle's router, as the flow calls it.
pub fn route_reticle(
    netlist: &Netlist,
    graph: &RoutingGraph,
    placement: &Placement,
    options: &RouteOptions,
) -> Result<(Routing, RunReport), String> {
    let rss_before_kib = start_memory();
    let started = Instant::now();
    let (routing, report) =
        route(netlist, graph, placement, options).map_err(|e| format!("reticle: {e}"))?;
    let seconds = started.elapsed().as_secs_f64();
    Ok((
        routing,
        RunReport {
            router: "reticle".into(),
            seconds,
            iterations: report.iterations.len() as u32,
            pips: report.pips,
            nodes: report.nodes,
            signals: report.signals,
            expansions: report.visited,
            rss_before_kib,
            rss_peak_kib: peak_memory(),
            threads: 1,
            ..RunReport::default()
        },
    ))
}

/// One signal as reticle's router sees it: where it starts and the graph nodes it has to
/// reach, each with the pin it is for.
struct Terminals {
    signal: usize,
    source: u32,
    sinks: Vec<(u32, usize)>,
}

fn terminals(
    netlist: &Netlist,
    graph: &RoutingGraph,
    placement: &Placement,
) -> Result<Vec<Terminals>, String> {
    let mut out = Vec::new();
    for signal in netlist.routable() {
        let s = &netlist.signals[signal];
        let driver = s.driver.ok_or("a routable signal has a driver")?;
        let pin = &netlist.pins[driver];
        let site = placement
            .site_of(pin.instance)
            .ok_or_else(|| format!("`{}` is not placed", netlist.instances[pin.instance].name))?;
        let source = graph.sites[site]
            .pin(&pin.role)
            .ok_or_else(|| format!("no wire for {}", pin.role))?;
        let mut sinks: Vec<(u32, usize)> = Vec::new();
        for &index in &s.sinks {
            let pin = &netlist.pins[index];
            let site = placement.site_of(pin.instance).ok_or_else(|| {
                format!("`{}` is not placed", netlist.instances[pin.instance].name)
            })?;
            for node in graph.sites[site].pin_nodes(&pin.role) {
                if node != source && !sinks.iter().any(|(n, _)| *n == node) {
                    sinks.push((node, index));
                }
            }
        }
        out.push(Terminals {
            signal,
            source,
            sinks,
        });
    }
    Ok(out)
}

/// Guide's router on the same problem, its routes turned into reticle's pips and replayed
/// through reticle's router to give a [`Routing`].
pub fn route_guide(
    netlist: &Netlist,
    graph: &RoutingGraph,
    placement: &Placement,
    options: &RouteOptions,
    config: &GuideConfig,
) -> Result<(Routing, RunReport), String> {
    let rss_before_kib = start_memory();
    let started = Instant::now();
    let terms = terminals(netlist, graph, placement)?;
    let mut fabric = Fabric::new(graph, options, config.threads);
    let entries: Vec<(u32, u32, u32)> = terms
        .iter()
        .flat_map(|t| t.sinks.iter().map(|(n, _)| (t.signal as u32, t.source, *n)))
        .collect();
    fabric.reserve_sink_entries(graph, &entries, &options.equivalent);
    fabric.per_tile = config.per_tile.unwrap_or(options.astar_weight);

    // A sink of a class of interchangeable inputs may end on any of them.
    let mut classes: HashMap<u32, Vec<u32>> = HashMap::new();
    for (node, class) in options.equivalent.iter().enumerate() {
        if *class != u32::MAX {
            classes.entry(*class).or_default().push(node as u32);
        }
    }

    let mut nets = Vec::with_capacity(terms.len());
    let mut rules = Vec::with_capacity(terms.len());
    // For each net and terminal, the graph node behind each access point's physical node.
    let mut behind: Vec<Vec<Vec<(u32, u32)>>> = Vec::with_capacity(terms.len());
    for (index, t) in terms.iter().enumerate() {
        let barred = !options
            .network_signals
            .get(t.signal)
            .copied()
            .unwrap_or(options.network_signals.is_empty());
        let strict = !barred
            && !t.sinks.is_empty()
            && t.sinks
                .iter()
                .all(|(_, pin)| options.network_pins.get(*pin) == Some(&true));
        let source = fabric.group[t.source as usize];
        rules.push(NetRules {
            barred,
            strict,
            source,
            signal: t.signal as u32,
        });
        let mut terminals = vec![Terminal::at(NodeId(source))];
        let mut names = vec![vec![(source, t.source)]];
        // Classes this net has more than one sink in are not offered as classes: two of its
        // sinks could end on one input.
        let class_of = |node: u32| {
            options
                .equivalent
                .get(node as usize)
                .copied()
                .unwrap_or(u32::MAX)
        };
        let mut seen: HashMap<u32, usize> = HashMap::new();
        for (node, _) in &t.sinks {
            *seen.entry(class_of(*node)).or_default() += 1;
        }
        for (node, _) in &t.sinks {
            let class = class_of(*node);
            let mut points: Vec<(u32, u32)> = vec![(fabric.group[*node as usize], *node)];
            if class != u32::MAX && seen[&class] == 1 {
                for other in &classes[&class] {
                    let g = fabric.group[*other as usize];
                    if *other != *node && !points.iter().any(|(p, _)| *p == g) {
                        points.push((g, *other));
                    }
                }
            }
            let access = points
                .iter()
                .map(|(g, _)| AccessPoint {
                    node: NodeId(*g),
                    cost: Cost::ZERO,
                })
                .collect();
            terminals.push(Terminal::new(access).map_err(|_| "an empty terminal")?);
            names.push(points);
        }
        nets.push(Net {
            id: NetId(index as u32),
            terminals,
        });
        behind.push(names);
    }
    fabric.rules = rules;
    let setup_seconds = started.elapsed().as_secs_f64();

    let mut engine = EngineOptions::default();
    let cost = |v: f64| Cost::new(v).unwrap_or(Cost::ZERO);
    engine.first_present = cost(
        config
            .first_present
            .unwrap_or(options.first_pass_present.unwrap_or(options.present_factor)),
    );
    engine.initial_present = cost(config.initial_present.unwrap_or(options.present_factor));
    engine.present_growth = cost(config.present_growth.unwrap_or(options.present_growth));
    engine.history_factor = cost(config.history_factor.unwrap_or(options.history_factor));
    engine.incremental_reroute = config.incremental;
    engine.bound_weight = config.bound_weight;
    engine.max_iterations = config.max_iterations;
    engine.batch_size = config.batch_size;
    engine.target_terminals = config.target_terminals;
    engine.timing_iterations = config.timing_iterations;
    // Timing-driven, by reticle's model, when asked and when the flow has one.
    let timed = options
        .timing
        .as_ref()
        .filter(|_| config.timed)
        .and_then(|t| t.paths.as_ref().map(|paths| (t, paths)));
    let outcome: Outcome<NodeId, NodeId> = match timed {
        Some((timing, paths)) => {
            fabric.set_delays(graph, &timing.node_delay, f64::from(timing.tile_delay));
            // A picosecond against a node of congestion as reticle weighs them: the mean
            // base cost of the timed nodes over their mean delay (`delay_scale`).
            let (mut bases, mut delays, mut count) = (0.0f64, 0.0f64, 0usize);
            for (node, delay) in timing.node_delay.iter().enumerate() {
                let base = f64::from(options.node_base.get(node).copied().unwrap_or(1.0));
                if *delay > 0 && base > 0.0 && !options.closed.get(node).copied().unwrap_or(false) {
                    bases += base;
                    delays += f64::from(*delay);
                    count += 1;
                }
            }
            let delay_cost = if count > 0 && delays > 0.0 {
                bases / delays * config.delay_weight
            } else {
                1e-3
            };
            let drivers: Vec<usize> = terms
                .iter()
                .map(|t| netlist.signals[t.signal].driver.unwrap_or(0))
                .collect();
            let sinks: Vec<Vec<usize>> = terms
                .iter()
                .map(|t| t.sinks.iter().map(|(_, pin)| *pin).collect())
                .collect();
            let driven = crate::timed::driven(
                netlist, graph, placement, paths, &drivers, &sinks, delay_cost,
            )?;
            if config.threads > 1 {
                guide::engine::route_timed_parallel(&fabric, &nets, engine, driven, config.threads)
            } else {
                guide::engine::route_timed(&fabric, &nets, engine, driven)
            }
        }
        None if config.threads > 1 => {
            guide::engine::route_parallel(&fabric, &nets, engine, config.threads)
        }
        None => guide::engine::route(&fabric, &nets, engine),
    }
    .map_err(|e| format!("guide: {e}"))?;
    let seconds = started.elapsed().as_secs_f64();
    let rss_peak_kib = peak_memory();
    eprintln!(
        "note: guide: {:?} after {} iteration(s), {seconds:.2} s ({setup_seconds:.2} s setup), {} expansion(s), {} physical node(s), {} step(s)",
        outcome.stop,
        outcome.routes.stats.iterations,
        outcome.routes.stats.nodes_expanded,
        fabric.len(),
        fabric.num_steps()
    );
    if outcome.stop != StopReason::Converged {
        return Err(format!(
            "guide: did not converge ({:?}), {} resource(s) overused after {} iteration(s)",
            outcome.stop,
            outcome.overused.len(),
            outcome.routes.stats.iterations
        ));
    }

    // Each net's route as reticle's pips.
    let mut chosen: Vec<Vec<u32>> = vec![Vec::new(); netlist.signals.len()];
    let mut used: Vec<bool> = vec![false; graph.nodes.len()];
    for (index, t) in terms.iter().enumerate() {
        let r = outcome
            .routes
            .route(NetId(index as u32))
            .ok_or_else(|| format!("guide left `{}` unrouted", netlist.signals[t.signal].name))?;
        let pips = pips_of(&fabric, graph, r, t.source, &behind[index])?;
        used[t.source as usize] = true;
        for pip in &pips {
            let p = graph.pip(*pip);
            used[p.from as usize] = true;
            used[p.to as usize] = true;
        }
        chosen[t.signal] = pips;
    }
    // guide's own result, checked before the replay: one signal a node, and every sink
    // walked back to the source through the signal's pips.
    {
        let mut owner: HashMap<u32, usize> = HashMap::new();
        let mut shared = 0usize;
        let mut unreached = 0usize;
        for t in &terms {
            let mut nodes: HashSet<u32> = HashSet::from([t.source]);
            let mut driven: HashMap<u32, u32> = HashMap::new();
            for pip in &chosen[t.signal] {
                let p = graph.pip(*pip);
                nodes.insert(p.to);
                driven.insert(p.to, p.from);
            }
            for n in nodes {
                let n = fabric.group[n as usize];
                if *owner.entry(n).or_insert(t.signal) != t.signal {
                    shared += 1;
                }
            }
            for (sink, _) in &t.sinks {
                let class = options
                    .equivalent
                    .get(*sink as usize)
                    .copied()
                    .unwrap_or(u32::MAX);
                let ends: Vec<u32> = if class == u32::MAX {
                    vec![*sink]
                } else {
                    classes[&class].clone()
                };
                let ok = ends.iter().any(|end| {
                    let mut at = *end;
                    let mut steps = 0;
                    while at != t.source && steps <= chosen[t.signal].len() {
                        match driven.get(&at) {
                            Some(from) => at = *from,
                            None => return false,
                        }
                        steps += 1;
                    }
                    at == t.source
                });
                if !ok {
                    unreached += 1;
                }
            }
        }
        if shared > 0 || unreached > 0 {
            eprintln!(
                "note: guide's routes: {shared} node(s) shared, {unreached} sink(s) not reached"
            );
        }
    }
    let mut report = RunReport {
        router: "guide".into(),
        seconds,
        setup_seconds,
        iterations: outcome.routes.stats.iterations as u32,
        pips: chosen.iter().map(Vec::len).sum(),
        nodes: used.iter().filter(|u| **u).count(),
        signals: terms.len(),
        expansions: outcome.routes.stats.nodes_expanded,
        rss_before_kib,
        rss_peak_kib,
        threads: config.threads,
        ..RunReport::default()
    };
    // The replay. Reticle's router is given a copy of the graph in which every pip guide did
    // not choose leads nowhere (it reads and drives the same node), but for the joins inside a
    // physical node, which are what makes it one: so each signal has only its own tree to
    // take, and pip IDs are those of the real graph, which the checks are then made on. The
    // nodes guide did not use are closed too.
    let mut chosen_pip = vec![false; graph.pips.len()];
    for pips in &chosen {
        for pip in pips {
            chosen_pip[*pip as usize] = true;
        }
    }
    let mut narrowed = graph.clone();
    for (index, pip) in narrowed.pips.iter_mut().enumerate() {
        if !chosen_pip[index] && fabric.group[pip.from as usize] != fabric.group[pip.to as usize] {
            pip.to = pip.from;
        }
    }
    drop(fabric);
    let replayed = Instant::now();
    let mut replay = options.clone();
    replay.closed = (0..graph.nodes.len())
        .map(|n| !used[n] || options.closed.get(n).copied().unwrap_or(false))
        .collect();
    replay.timing = None;
    replay.on_iteration = None;
    let routed = route(netlist, &narrowed, placement, &replay);
    report.replay_seconds = replayed.elapsed().as_secs_f64();
    drop(narrowed);
    match routed {
        Ok((routing, _)) => {
            report.exact = Some(compare(&routing, &chosen, &terms));
            Ok((routing, report))
        }
        Err(e) => {
            report.replay_error = Some(format!("the replay of guide's routes: {e}"));
            Ok((Routing::new(netlist.signals.len()), report))
        }
    }
}

/// How many signals `routing` routes with exactly guide's pips, of how many.
fn compare(routing: &Routing, chosen: &[Vec<u32>], terms: &[Terminals]) -> (usize, usize) {
    let mut same = 0;
    for t in terms {
        let mut a: Vec<u32> = routing.pips(t.signal).to_vec();
        let mut b = chosen[t.signal].clone();
        a.sort_unstable();
        b.sort_unstable();
        if a == b {
            same += 1;
        }
    }
    (same, terms.len())
}

/// A route over physical nodes as reticle's pips: the step pip of each edge, and inside each
/// physical node the joins from the name it was entered by to every name it is left by or
/// ends on.
fn pips_of(
    fabric: &Fabric,
    graph: &RoutingGraph,
    route: &guide::net::Route<NodeId>,
    source: u32,
    behind: &[Vec<(u32, u32)>],
) -> Result<Vec<u32>, String> {
    let mut pips = Vec::new();
    // Per physical node of the route: the name it is entered by, and the names it must reach.
    let mut entered: HashMap<u32, u32> = HashMap::new();
    let mut needed: HashMap<u32, Vec<u32>> = HashMap::new();
    entered.insert(fabric.group[source as usize], source);
    for edge in route.edges() {
        let pip = fabric
            .step_pip(edge.from.0, edge.to.0)
            .ok_or("a route's step is not a step of the fabric")?;
        let p = graph.pip(pip);
        pips.push(pip);
        entered.insert(edge.to.0, p.to);
        needed.entry(edge.from.0).or_default().push(p.from);
    }
    let on: HashSet<u32> = route.nodes().map(|n| n.0).collect();
    for points in behind.iter().skip(1) {
        if let Some((g, name)) = points.iter().find(|(g, _)| on.contains(g)) {
            needed.entry(*g).or_default().push(*name);
        }
    }
    let mut keys: Vec<u32> = needed.keys().copied().collect();
    keys.sort_unstable();
    for g in keys {
        let from = *entered
            .get(&g)
            .ok_or("a physical node of a route was never entered")?;
        let wanted = &needed[&g];
        if wanted.iter().all(|w| *w == from) {
            continue;
        }
        // Breadth first over the joins inside the physical node.
        let mut parent: HashMap<u32, u32> = HashMap::new();
        let mut queue = VecDeque::from([from]);
        let mut seen: HashSet<u32> = HashSet::from([from]);
        while let Some(at) = queue.pop_front() {
            for pip in graph.outgoing(at) {
                let to = graph.pip(*pip).to;
                if fabric.group[to as usize] == g && seen.insert(to) {
                    parent.insert(to, *pip);
                    queue.push_back(to);
                }
            }
        }
        let mut taken: HashSet<u32> = HashSet::new();
        for w in wanted {
            let mut at = *w;
            while at != from {
                let pip = *parent.get(&at).ok_or(
                    "a name of a physical node is not joined to the one it was entered by",
                )?;
                if !taken.insert(pip) {
                    break;
                }
                pips.push(pip);
                at = graph.pip(pip).from;
            }
        }
    }
    let _ = fabric.names(0);
    Ok(pips)
}
