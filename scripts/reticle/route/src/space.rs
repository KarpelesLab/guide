//! Reticle's routing graph as a guide [`RoutingSpace`].
//!
//! A node of the space is one **physical node** of reticle's graph
//! (`reticle::fpga::route::physical_nodes`): the names one piece of metal has in the tiles it
//! crosses, which reticle's loaders join with a bit-less pip each way. Reticle's router keeps
//! capacity per physical node and refuses a route that drives one twice
//! (`Routing::verify`, "drives ... from more than one place"); a tree over physical nodes can
//! do neither, so the rule is kept by construction rather than checked. A step is a pip
//! between two physical nodes, the first one met for each pair, preferring one that more
//! nets may take; a route is turned back into reticle's pips by [`Fabric::pips_of`], which
//! adds the joins inside each physical node from where it was entered to where it is left.
//!
//! What reticle's router enforces per signal is enforced here per net, from the same
//! [`RouteOptions`] the family's flow builds:
//!
//! - `closed` nodes are entered by no net, and `network` nodes only by the nets
//!   `network_signals` allows (a physical node with any such name counts as such).
//! - `network_entry`: a net allowed onto the network enters such a node through a
//!   programmable pip only from a node of the network.
//! - `network_pins`: a net all of whose sinks are such pins takes no programmable pip out of
//!   a node that is not the network's, but out of its source. Reticle holds each sink to it on
//!   its own; a net of both kinds of sink is not held to it here.
//! - The only ways into a sink are reserved for its signal, at reticle's price
//!   (`reserve_sink_entries`, a penalty of 10 000 times the step).
//!
//! For timing-driven routing it is a [`DelaySpace`] too: a step's delay is reticle's
//! `node_delay` of the graph node its pip enters (under reticle's fitted model, a node a join
//! enters has none), and the lower bound on delay is reticle's `tile_delay` a tile.
//!
//! What it does not do: reticle's bounding box, `first` (routing some pins of a net before
//! others), a branch's load on the connections past it, and a critical search's budget.

use std::collections::HashMap;

use guide::cost::Cost;
use guide::id::{NetId, NodeId};
use guide::space::{DelaySpace, DenseSlots, RoutingSpace, Step};
use reticle::fpga::arch::RoutingGraph;
use reticle::fpga::route::{RouteOptions, physical_nodes};

/// The penalty reticle charges for a sink's only way in to every other signal.
const RESERVED_PENALTY: f64 = 1.0e4;

/// A step between physical nodes, and what the expansion needs to know of its pip.
#[derive(Clone, Copy, Debug)]
struct StepInfo {
    to: u32,
    pip: u32,
    cost: f32,
    /// The pip has configuration bits: it is a choice, not a fixed connection.
    programmable: bool,
    /// The pip leaves a node of the network.
    from_network: bool,
    /// The pip enters a `network_entry` node.
    to_entry: bool,
}

/// What a net may and may not do.
#[derive(Clone, Copy, Debug, Default)]
pub struct NetRules {
    /// Kept off the network.
    pub barred: bool,
    /// Held to the network: every programmable pip leaves a network node or the source.
    pub strict: bool,
    /// Its source's physical node.
    pub source: u32,
    /// Its signal, for the reserved entries.
    pub signal: u32,
}

/// Reticle's graph, as physical nodes and the steps between them.
pub struct Fabric {
    /// Each graph node's physical node, numbered densely from 0.
    pub group: Vec<u32>,
    /// The graph nodes of each physical node.
    names_start: Vec<u32>,
    names: Vec<u32>,
    steps_start: Vec<u32>,
    steps: Vec<StepInfo>,
    /// Per physical node: any name on the network, or closed.
    network: Vec<bool>,
    closed: Vec<bool>,
    /// What the lower bound scales a tile by at each physical node: its least base cost, as
    /// reticle's estimate does (`heuristic_base`: a node that costs nothing counts as one).
    scale: Vec<f32>,
    /// The tiles each physical node covers.
    bbox: Vec<[u16; 4]>,
    /// The sink entries reserved for a signal, by physical node: the signal, or `u32::MAX`
    /// when two signals' sinks share it.
    reserved: HashMap<u32, u32>,
    /// One bit a physical node, set when it is in `reserved`, so that a step looks the map
    /// up only then.
    reserved_bits: Vec<u64>,
    /// What each net may do, by net ID.
    pub rules: Vec<NetRules>,
    /// The cost of a tile of distance, in the lower bound.
    pub per_tile: f64,
    /// Each step's delay, for timing-driven routing, and a tile's in the lower bound on it.
    delays: Vec<u32>,
    tile_delay: f64,
}

impl Fabric {
    /// The physical nodes of `graph`, under `options`.
    pub fn new(graph: &RoutingGraph, options: &RouteOptions, threads: usize) -> Fabric {
        let n = graph.nodes.len();
        let roots = physical_nodes(graph);
        // Dense numbers for the roots, in the order of their least node.
        let mut dense = vec![u32::MAX; n];
        let mut count = 0u32;
        for &root in &roots {
            let root = root as usize;
            if dense[root] == u32::MAX {
                dense[root] = count;
                count += 1;
            }
        }
        let group: Vec<u32> = roots.iter().map(|root| dense[*root as usize]).collect();
        drop(roots);
        drop(dense);
        let r = count as usize;
        let mut names_start = vec![0u32; r + 1];
        for g in &group {
            names_start[*g as usize + 1] += 1;
        }
        for i in 0..r {
            names_start[i + 1] += names_start[i];
        }
        let mut fill = names_start.clone();
        let mut names = vec![0u32; n];
        for (node, g) in group.iter().enumerate() {
            names[fill[*g as usize] as usize] = node as u32;
            fill[*g as usize] += 1;
        }
        drop(fill);

        let flag = |v: &[bool], i: usize| v.get(i).copied().unwrap_or(false);
        let mut network = vec![false; r];
        let mut closed = vec![false; r];
        let mut scale = vec![f32::INFINITY; r];
        let mut bbox = vec![[u16::MAX, u16::MAX, 0u16, 0u16]; r];
        let (w, h) = (graph.width.max(1) - 1, graph.height.max(1) - 1);
        for (node, wire) in graph.nodes.iter().enumerate() {
            let g = group[node] as usize;
            network[g] |= flag(&options.network, node);
            closed[g] |= flag(&options.closed, node);
            let base = options.node_base.get(node).copied().unwrap_or(1.0);
            scale[g] = scale[g].min(if base == 0.0 { 1.0 } else { base });
            let (x0, x1, y0, y1) = if wire.global {
                (0, w, 0, h)
            } else {
                let ends = |at: u32, span: i32, max: u32| {
                    let other = (i64::from(at) + i64::from(span)).clamp(0, i64::from(max)) as u32;
                    (at.min(other), at.max(other))
                };
                let (x0, x1) = ends(wire.tile.0, wire.span.0, w);
                let (y0, y1) = ends(wire.tile.1, wire.span.1, h);
                (x0, x1, y0, y1)
            };
            let b = &mut bbox[g];
            b[0] = b[0].min(x0 as u16);
            b[1] = b[1].min(y0 as u16);
            b[2] = b[2].max(x1 as u16);
            b[3] = b[3].max(y1 as u16);
        }

        // The steps out of each physical node, one for each physical node it reaches.
        // Over threads when there are any, each taking a run of physical nodes; joined in
        // order, so the result is the same.
        let threads = threads.clamp(1, 64);
        let chunk = r.div_ceil(threads).max(1);
        let ctx = StepContext {
            graph,
            options,
            group: &group,
            names: &names,
            names_start: &names_start,
            r,
        };
        let parts: Vec<(Vec<u32>, Vec<StepInfo>)> = if threads == 1 {
            vec![ctx.steps(0..r)]
        } else {
            std::thread::scope(|scope| {
                let ctx = &ctx;
                let handles: Vec<_> = (0..r)
                    .step_by(chunk)
                    .map(|start| scope.spawn(move || ctx.steps(start..(start + chunk).min(r))))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("a thread building steps panicked"))
                    .collect()
            })
        };
        let mut steps_start = Vec::with_capacity(r + 1);
        let mut steps: Vec<StepInfo> = Vec::with_capacity(parts.iter().map(|(_, s)| s.len()).sum());
        for (starts, part) in parts {
            let base = steps.len() as u32;
            steps_start.extend(starts.iter().map(|s| s + base));
            steps.extend(part);
        }
        steps_start.push(steps.len() as u32);
        steps.shrink_to_fit();

        Fabric {
            group,
            names_start,
            names,
            steps_start,
            steps,
            network,
            closed,
            scale,
            bbox,
            reserved: HashMap::new(),
            reserved_bits: vec![0; r.div_ceil(64)],
            rules: Vec::new(),
            per_tile: 0.0,
            delays: Vec::new(),
            tile_delay: 0.0,
        }
    }

    /// How many physical nodes there are.
    pub fn len(&self) -> usize {
        self.network.len()
    }

    /// How many steps there are between them.
    pub fn num_steps(&self) -> usize {
        self.steps.len()
    }

    /// The graph nodes of one physical node.
    pub fn names(&self, g: u32) -> &[u32] {
        &self.names
            [self.names_start[g as usize] as usize..self.names_start[g as usize + 1] as usize]
    }

    /// Marks, for each sink, the nodes in front of it that are its only way in, as reticle's
    /// `reserve_sink_entries` does: walking back from the sink while a node has one way in
    /// from another physical node.
    pub fn reserve_sink_entries(
        &mut self,
        graph: &RoutingGraph,
        sinks: &[(u32, u32, u32)],
        equivalent: &[u32],
    ) {
        for &(signal, source, sink) in sinks {
            if equivalent
                .get(sink as usize)
                .is_some_and(|class| *class != u32::MAX)
            {
                continue;
            }
            let mut node = sink;
            for _ in 0..8 {
                let root = self.group[node as usize];
                self.reserved_bits[root as usize / 64] |= 1 << (root % 64);
                let owner = self.reserved.entry(root).or_insert(signal);
                if *owner != signal {
                    *owner = u32::MAX;
                }
                let mut names = vec![node];
                let mut at = 0;
                while at < names.len() && names.len() < 16 {
                    for pip in graph.incoming(names[at]) {
                        let from = graph.pip(*pip).from;
                        if self.group[from as usize] == root && !names.contains(&from) {
                            names.push(from);
                        }
                    }
                    at += 1;
                }
                let mut ways = names
                    .iter()
                    .flat_map(|name| graph.incoming(*name))
                    .map(|pip| graph.pip(*pip).from)
                    .filter(|from| self.group[*from as usize] != root)
                    .filter(|from| *from == source || !graph.incoming(*from).is_empty());
                match (ways.next(), ways.next()) {
                    (Some(from), None) => node = from,
                    _ => break,
                }
            }
        }
    }

    /// The tiles between two physical nodes, as a Manhattan gap between what they cover.
    fn gap(&self, a: u32, b: u32) -> u32 {
        let (p, q) = (self.bbox[a as usize], self.bbox[b as usize]);
        // At most one of each pair is above zero: the boxes are apart on that axis one way,
        // the other way, or not at all.
        let dx = q[0].saturating_sub(p[2]) + p[0].saturating_sub(q[2]);
        let dy = q[1].saturating_sub(p[3]) + p[1].saturating_sub(q[3]);
        u32::from(dx) + u32::from(dy)
    }

    /// Whether a net under `rules` may take `step` out of `from`.
    #[inline]
    fn allowed(&self, rules: &NetRules, from: u32, step: &StepInfo) -> bool {
        let to = step.to as usize;
        !(self.closed[to]
            || (rules.barred && self.network[to])
            || (!rules.barred && step.to_entry && !step.from_network && step.programmable)
            || (rules.strict && from != rules.source && !step.from_network && step.programmable))
    }

    /// Gives each step the delay of the graph node its pip enters, from reticle's
    /// `Timing::node_delay`, and the lower bound on delay a tile of distance.
    pub fn set_delays(&mut self, graph: &RoutingGraph, node_delay: &[u32], tile_delay: f64) {
        self.delays = self
            .steps
            .iter()
            .map(|step| {
                node_delay
                    .get(graph.pip(step.pip).to as usize)
                    .copied()
                    .unwrap_or(0)
            })
            .collect();
        self.tile_delay = tile_delay;
    }

    /// The pip of the step from one physical node to another.
    pub fn step_pip(&self, from: u32, to: u32) -> Option<u32> {
        let range =
            self.steps_start[from as usize] as usize..self.steps_start[from as usize + 1] as usize;
        self.steps[range].iter().find(|s| s.to == to).map(|s| s.pip)
    }
}

/// What building the steps of a run of physical nodes reads.
struct StepContext<'a> {
    graph: &'a RoutingGraph,
    options: &'a RouteOptions,
    group: &'a [u32],
    names: &'a [u32],
    names_start: &'a [u32],
    r: usize,
}

fn flag_of(v: &[bool], i: usize) -> bool {
    v.get(i).copied().unwrap_or(false)
}

impl StepContext<'_> {
    /// The steps out of the physical nodes of `range`, and where each one's start, from 0.
    fn steps(&self, range: std::ops::Range<usize>) -> (Vec<u32>, Vec<StepInfo>) {
        let mut steps_start = Vec::with_capacity(range.len());
        let pips: usize = self.names
            [self.names_start[range.start] as usize..self.names_start[range.end] as usize]
            .iter()
            .map(|name| self.graph.outgoing(*name).len())
            .sum();
        let mut steps: Vec<StepInfo> = Vec::with_capacity(pips);
        // Per physical node reached, the physical node it was last reached from and where its
        // step is: a table and not a map, since this runs over every pip of the part.
        let mut seen: Vec<(u32, u32)> = vec![(u32::MAX, 0); self.r];
        // A name's pips are read in passes, each over all of them, so that their reads of
        // memory do not wait on one another.
        let mut ahead: Vec<(u32, u32, u32)> = Vec::new();
        for g in range {
            steps_start.push(steps.len() as u32);
            for &name in &self.names[self.names_start[g] as usize..self.names_start[g + 1] as usize]
            {
                let from_network = flag_of(&self.options.network, name as usize);
                ahead.clear();
                ahead.extend(
                    self.graph
                        .outgoing(name)
                        .iter()
                        .map(|&pip| (pip, self.graph.pip(pip).to, 0)),
                );
                for entry in &mut ahead {
                    entry.2 = self.group[entry.1 as usize];
                }
                for &(pip, to_node, to) in &ahead {
                    if to as usize == g {
                        continue;
                    }
                    let info = StepInfo {
                        to,
                        pip,
                        cost: self
                            .options
                            .node_base
                            .get(to_node as usize)
                            .copied()
                            .unwrap_or(1.0),
                        programmable: !self.graph.pip_bits(pip).is_empty(),
                        from_network,
                        to_entry: flag_of(&self.options.network_entry, to_node as usize),
                    };
                    let slot = &mut seen[to as usize];
                    if slot.0 != g as u32 {
                        *slot = (g as u32, steps.len() as u32);
                        steps.push(info);
                    } else {
                        // The one fewer nets are barred from: a fixed connection over a
                        // choice, then a pip out of the network, then the cheaper.
                        let at = slot.1 as usize;
                        let rank = |s: &StepInfo| {
                            (
                                s.programmable,
                                !s.from_network,
                                s.to_entry,
                                (s.cost * 1e6) as i64,
                            )
                        };
                        if rank(&info) < rank(&steps[at]) {
                            steps[at] = info;
                        }
                    }
                }
            }
        }
        (steps_start, steps)
    }
}

impl RoutingSpace for Fabric {
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

    #[inline]
    fn contains(&self, node: NodeId) -> bool {
        (node.0 as usize) < self.network.len()
    }

    fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
        let rules = self.rules.get(net.0 as usize).copied().unwrap_or_default();
        let g = from.0 as usize;
        let range = self.steps_start[g] as usize..self.steps_start[g + 1] as usize;
        for step in &self.steps[range] {
            if !self.allowed(&rules, from.0, step) {
                continue;
            }
            let to = step.to as usize;
            let mut cost = f64::from(step.cost);
            if self.reserved_bits[to / 64] >> (to % 64) & 1 != 0
                && let Some(owner) = self.reserved.get(&step.to)
                && *owner != rules.signal
                && *owner != u32::MAX
            {
                cost *= RESERVED_PENALTY;
            }
            out.push(Step {
                to: NodeId(step.to),
                cost: Cost::new(cost).unwrap_or(Cost::ZERO),
            });
        }
    }

    #[inline]
    fn lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> Cost {
        let scale = f64::from(self.scale[from.0 as usize]);
        Cost::new(self.per_tile * scale * f64::from(self.gap(from.0, to.0))).unwrap_or(Cost::ZERO)
    }

    #[inline]
    fn resources(&self, _net: NetId, _from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
        out.push(to);
    }

    #[inline]
    fn capacity(&self, _resource: NodeId) -> u32 {
        1
    }
}

impl DelaySpace for Fabric {
    fn step_delays(&self, net: NetId, from: NodeId, out: &mut Vec<u32>) {
        let rules = self.rules.get(net.0 as usize).copied().unwrap_or_default();
        let g = from.0 as usize;
        let range = self.steps_start[g] as usize..self.steps_start[g + 1] as usize;
        for (index, step) in self.steps[range.clone()].iter().enumerate() {
            if self.allowed(&rules, from.0, step) {
                out.push(self.delays.get(range.start + index).copied().unwrap_or(0));
            }
        }
    }

    #[inline]
    fn delay_lower_bound(&self, _net: NetId, from: NodeId, to: NodeId) -> u32 {
        (self.tile_delay * f64::from(self.gap(from.0, to.0))) as u32
    }
}
