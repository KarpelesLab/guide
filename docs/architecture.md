# Architecture

This records the intended design, and it changes as implementation teaches us things.
The core model, search, multi-net engine, validator, timing analysis, geometry layer,
PCB routing and generic placement exist; see the [roadmap](../ROADMAP.md) for what doesn't yet. Code sketches are
illustrative, not an API.

## Layers

```text
 ┌───────────────────────────────────────────────────────────┐
 │ Interchange     DSN/SES, KiCad, VTR rr_graph, ...         │
 ├───────────────────────────────────────────────────────────┤
 │ Domains         pcb, fpga, grid, ...                      │
 │                 implement the spaces, rules and costs     │
 ├────────────────────────────┬──────────────────────────────┤
 │ Routing engines            │ Placement engines            │
 │ negotiated congestion,     │ annealing, analytic,         │
 │ rip-up and reroute         │ legalization                 │
 ├────────────────────────────┴──────────────────────────────┤
 │ Shared algorithms   search, Steiner trees, timing         │
 ├───────────────────────────────────────────────────────────┤
 │ Core model          nets, terminals, costs, RoutingSpace  │
 └───────────────────────────────────────────────────────────┘
   Geometry layer: built on polyclip, used by PCB-like domains only
   Validator: depends on the core model and domains, never on the engines
```

Dependencies point downward only. Engines never name a domain type. Domains that
have no geometry (FPGA) do not pay for the geometry layer.

These start as modules in the single `guide` crate, with optional domains and
interchange formats behind Cargo features. We can split into a workspace later if
compile times or release cadence call for it. So far: `fpga` for VTR's formats, `pcb` for
the geometry layer, `kicad`, which implies `pcb`, for reading KiCad boards into it, `dsn`,
which implies `pcb` too, for reading Specctra designs into it and writing sessions from it,
`mapf` for multi-agent path finding, a time-expanded space that the engine routes agents
through unchanged, with a reader for the MovingAI benchmarks, and `asic` for ASIC global
routing on the ISPD contests' designs.

## Target consumers

Two existing crates are expected to adopt `guide`, and they constrain its design:

- [reticle](https://github.com/KarpelesLab/reticle), an HDL compiler with its own
  FPGA placer and router. Its contributing rules forbid third-party crates, and
  all of its current dependencies are optional.
- [cadlab](https://github.com/KarpelesLab/cadlab), a headless PCB CAD tool with its
  own autorouter, built on [polyclip](https://github.com/KarpelesLab/polyclip).

Both currently declare Rust 1.89 and will move to `guide`'s 1.95 before adopting it.

## Core abstractions

### `RoutingSpace`

The central abstraction, and the one most likely to be wrong the first time. An
engine has to answer five questions, and a domain answers them however it likes. It
also has to say how to keep flat arrays of state for its nodes and its resources:

```rust
pub trait RoutingSpace {
    /// A search state: a device wire, or (position, layer, direction) on a board.
    type Node: Copy + Eq + Hash + Debug;
    /// Something nets compete for.
    type Resource: Copy + Eq + Ord + Hash + Debug;
    /// How search state is keyed: `DenseSlots` or `InternedSlots`, or a mapping of its own.
    type Slots: NodeSlots<Self::Node>;
    /// How negotiation state is keyed, in the same way.
    type ResourceSlots: NodeSlots<Self::Resource>;

    fn new_slots(&self) -> Self::Slots;
    fn new_resource_slots(&self) -> Self::ResourceSlots;

    /// Does this node exist at all? A blocked cell does, and a number past the end doesn't.
    fn contains(&self, node: Self::Node) -> bool;

    /// Where can a route for this net go next, and at what base cost?
    fn expand(&self, net: NetId, from: Self::Node, out: &mut Vec<Step<Self::Node>>);

    /// A lower bound on the remaining cost, for A*. Must never overestimate.
    fn lower_bound(&self, net: NetId, from: Self::Node, to: Self::Node) -> Cost;

    /// Which resources does taking this step consume?
    fn resources(&self, net: NetId, from: Self::Node, to: Self::Node, out: &mut Vec<Self::Resource>);

    /// How many nets can share this resource before it counts as overused?
    fn capacity(&self, resource: Self::Resource) -> u32;
}

pub trait NodeSlots<N> {
    /// The slot for this key, assigning one if needed. For a search, which owns its mapping.
    fn slot(&mut self, key: N) -> usize;
    /// The slot for this key if it has one, changing nothing. For pricing, which shares it.
    fn get(&self, key: N) -> Option<usize>;
    /// Forgets assignments, ready for a new search.
    fn clear(&mut self);
}
```

The two main implementations sit at opposite extremes, and the abstraction has to
serve both:

| | FPGA fabric | PCB |
| --- | --- | --- |
| Node | Index into a prebuilt graph | Point on a layer, generated lazily |
| Expansion | Read an adjacency list | Geometric query against the design rules |
| Resource | The wire itself, naturally discrete | Tile sides globally; in detail, conflicts found by exact clearance queries ([decision](#decisions)) |
| Graph size | Up to hundreds of millions of nodes, so compact storage is essential | Effectively unbounded, so it can't be stored at all |

### Nets and routes

A **net** has terminals. Each terminal offers one or more equivalent access points,
which covers FPGA pin equivalence, through-hole pads reachable from any layer, and
swappable pins. A **route** is a tree over steps in the space. Routes are plain data
and can be validated without the router.

### Engines

A routing engine takes a space, a set of nets and a budget, and returns a result
holding routes, unrouted nets with reasons, and statistics. It is a state machine
that can report progress and stop between iterations, so callers can cancel at any
time and keep the best result so far.

Placement engines mirror this through a `PlacementProblem` of items, legal locations
and a cost. They query routing engines for congestion estimates through a narrow
interface, so that the two can be iterated together (roadmap M9). In `src/place`: the annealer
steps a temperature at a time with a budget and a cancel flag, as the routing engine steps
iterations, and the interface is `CongestionEstimate`, an overflow for a rectangle, which a
RUDY map or a router's own demand can answer. Placement costs are `u64` sums over nets of a
`CostTerm`, cached per net so that a move prices only the nets it touches.

Domains build problems for these placers rather than placers of their own: `fpga` from a
packed netlist and a device, and `pcb::place` from a board's components, with the board's
edge, fixed parts and keepouts rasterized into fixed items of a free-space problem, and its two
sides placed in passes, each side's parts real in turn and the other's fixed or taking no room.
`src/place` needed nothing new for either.

## Decisions

Each of these can be revisited, but changing one should be a conscious act.

**Numeric types follow the consumers.** Conversions at the crate boundary should be
free and infallible wherever possible:

| Quantity | Type | Why |
| --- | --- | --- |
| Layout coordinates | `i64` database units (nanometres on PCBs), limited to ±2^40 | Matches polyclip, cadlab's `Nm` and reticle's DEF points. The limit is polyclip's `MAX_COORD`, about 1.1 km in nanometres, and such values convert to `f64` exactly. |
| Exact geometry checks | `i128` and wider, through polyclip | Products of `i64` coordinates do not fit in `i64`. |
| Approximate geometry | `f64` | Used for search and filtering only, and never stored in results. The validator has the final word. |
| Routing costs | `Cost`, a non-negative, non-NaN `f64` | Tuning values are fractional. The invariant lets queues compare `to_bits()` as a `u64`, as reticle does. |
| Placement costs | `u64` | Matches reticle's half-perimeter cost. |
| Delays | `u32` picoseconds per element, `u64` for path totals | Matches reticle's timing data. |
| Criticality, temperature | `f64` | These are ratios. |
| Node, edge and net IDs | `u32` newtypes | Keeps graph storage compact. Matches reticle's `NodeId` and `PipId` and cadlab's grid nodes. |
| FPGA tile positions | `u32`, with `i32` spans | Matches reticle's routing graph. |
| Rotation | `i32` millidegrees | Matches cadlab's `Angle`. |
| Seeds | `u64` | Matches reticle's placer. |

**Floats are allowed in results-affecting code, under rules.** Addition, subtraction,
multiplication, division and `sqrt` are deterministic for a fixed order of operations,
so the order must be fixed. `exp`, `powf` and the trigonometric functions can differ
between platforms, so anything that affects results (the annealer's acceptance test,
for example) needs an implementation of our own.

**Determinism includes parallelism.** Results must not depend on thread count or
scheduling. A fixed number of nets, the batch size, route at once against a frozen
state of the resource table, and one thread then commits them in net order. A route
that uses what an earlier commit of its batch has taken is thrown away, and the net
routes again first in the next batch, after the rest of its batch. Batch membership
depends only on the commits, and a route only on the table, so threads have no say in
anything. Budgets are charged during the same walk through the nets, so that threads
cannot race for them.

Batch size is therefore an option that changes results, and thread count is one that
cannot. A batch size of 1, the default, is exactly the sequential router, and any batch
size can run on one thread for a space that threads cannot share. Each thread costs a
full set of search state, 44 bytes per node or about 13 GB at 300 million nodes, so
threads are the caller's to ask for and the default is one.

This replaces a plan to batch nets with disjoint bounding boxes, because that assumption
did not survive measuring: 31% of routes on a congested fabric leave their terminals'
box, and packing such nets by box left a limit of 1.4 times the speed at any thread count.
Spaces also have no notion of region. Checking everything a search read, rather than only
the route it found, reproduces sequential results at any batch size, but on hard problems
it was slower than one thread. Keeping the order of the nets within a batch, so that the
routes after a dropped one waited for it, was also tried and dropped: on FPGA graphs, where
a fifth of routes are dropped, it left batches of 32 with 4 to 6 new nets each, and 16
threads slower than one. Batches of 32 add about 0.1% to route cost on fabrics and sparse
grids, at most 1.5% on any one problem, and about 0.15% to wire and to delay on VPR's
graphs. Dense grids of crossing nets, where every net fights every other, take more
iterations in batches. Batches reach 2.7 to 3.7 times the speed at 16 threads on problems
that take seconds, and about twice it on the larger FPGA circuits, more with batches of
64 or 128. The limit is mostly that a few searches are much bigger than the rest, and a
net is the unit of work. The [engine's documentation](../src/engine.rs) has the
measurements.

Hash maps with randomized iteration order are not allowed in any path that affects
results. Debugging, regression testing and benchmark comparison all rely on this, and
retrofitting it is close to impossible.

**The validator is independent.** A bug shared by router and checker is invisible,
so the validator shares the problem model only. On PCBs, we also cross-check against
KiCad's own DRC: the clearance check agrees with KiCad's on nine real boards, but for
differences that are each explained ([results](benchmarks.md#pcb-clearance-checks-against-kicad)).

**Struct-of-arrays and indices, not pointers.** Large FPGA graphs make cache
behaviour a first-order concern. Nodes and nets are newtyped integer indices, which
also keeps the borrow checker out of graph algorithms.

**No dependencies in the core.** The default build has no external crates and no
external solver, which keeps it usable from reticle. Parsers and optional
integrations go behind features, and are written here, with no dependencies either: the
KiCad and DSN readers share an S-expression reader that knows both dialects, and the KiCad
reader has its own JSON parser.

**Geometry uses polyclip, but does not expose it.** polyclip already provides polygon
booleans, offsetting, arcs and exact distance queries, so a feature-gated geometry
layer builds on it rather than reimplementing them. Its types stay out of the public
API: cargo treats each polyclip 0.0.x release as breaking, so exposing them would force
`guide` and its consumers to upgrade in lockstep. `guide` has its own
`Point { x: i64, y: i64 }`, Y-up like polyclip and cadlab, with conversions behind the
feature.

**PCB routing negotiates over tiles globally, and resolves conflicts by rip-up in detail.**
This settles open questions 1 and 4, for M5, as `src/pcb` does it:

- *Global routing* is a `RoutingSpace` of square tiles on each layer, 16 lattice pitches on a
  side, run by the unchanged negotiated-congestion engine. Its resources are discrete, as the
  engine wants: the side between two tiles on a layer, whose capacity is how many tracks fit
  along its free length, sampled against the fixed copper, and a tile's vias. Each net's
  global route, grown by a tile, is its corridor.
- *Detailed routing* is a second `RoutingSpace`, of points on layers generated as a search
  reaches them and kept in `InternedSlots`, searched one net at a time with
  `search::Search` (not the engine), inside the net's corridor. It has no resources at all:
  every step is legal or not by exact clearance queries against a `ShapeIndex` of the fixed
  copper and the routes committed so far, by the same rules as `drc::check`. So conflicts
  between nets can't arise in a committed result. A net that finds no path is searched for
  again with other nets' routes passable at a price, raised by a history of conflicts kept
  per tile, and the nets it crosses are ripped up and routed again. That is the "resources
  derived from conflicts, with history in a spatial structure" option, run outside the
  engine.
- So `RoutingSpace` needed no change, and the FPGA and grid spaces and their results are
  untouched. What the engine would have needed, to negotiate the detailed level too, is a
  conflict query in place of discrete resources, since "within a clearance of" has no
  discrete identity. That stays open until a board shows that rip-up isn't enough.

The measurements behind it, on the nine [curated boards](../testdata/kicad/README.md)
([details](benchmarks.md#pcb-routing-of-the-curated-boards)):

- *Rip-up is what completes boards.* Without it, 4 of 9 boards are complete; with it, 8.
  `GB-LIVE32` goes from 136 to 197 of 197 connections, `udb-s` from 21 to 33 of 33, `flea`
  from 57 to 79 of 79.
- *Global routing helps completion, but not by restricting layers.* Steering detailed routing
  onto the layers that global routing chose, by making other layers 20% dearer, lost nets on
  both hard boards (`GB-LIVE32` 150 connections against 197, `nk3-nrf-mini` 54 against 56),
  so that is off. Corridors alone completed `GB-LIVE32` where routing without them didn't
  (158 of 197) and did slightly better on `nk3-nrf-mini` (56 against 55), in about the same
  time, within 10% either way, but with 2% to 6% more track on three of the complete boards,
  where global routing makes nets detour around congestion that detailed routing would have
  resolved. Corridors with a margin of 0 or 2 tiles did worse on both hard boards than 1.
- *A second, finer pass for what is left* completed `GB-LIVE32`: its 40-pad ground net, last
  routed, fits on a lattice of half the pitch. It costs nothing on boards that don't need it.
- *Results are chaotic on the hard boards*, as with any sequential router: a small change of
  option moves `nk3-nrf-mini` between 23 and 28 of 30 nets. Conclusions above come from
  several boards agreeing, not from one.

**Differential pairs are routed side by side last, in place of their nets' routes.** Routing
pairs first, before anything else, as fixed copper, coupled `udb-s`'s USB pair a little better,
60% of its length side by side in place of 50%, but cost other nets their room: `GB-LIVE32`
lost a net to its USB pair, and routing it again with the pair as two nets doubled its time.
Routed last, each pair is taken out, routed side by side
where it fits, and kept only if it connects what its two routes did, so no other net changes
at all, and a pair that doesn't fit stays as two nets. Meanders come after that, for the same
reason ([results](benchmarks.md#pcb-differential-pairs-and-lengths)). On the DRAM boards,
though, routed last, no pair finds room through the buses around the BGA, even with the routes
in its way ripped up and routed again, which was tried; so `Options::pairs_first` routes them
first, before anything else, as fixed copper, for boards whose pairs matter most
([results](benchmarks.md#pcb-routing-of-dram-boards)). Fixed, they can wall a pad in, so
while they are routed, a pad of a net with zones beside a pair's pads keeps a place for its via
free of them. Moving pairs out of the way of a net left at the end, and routing them again
around it, was tried and dropped: by then no pair finds room again among routes that can't
move. Nor was a second run with the nets left routed first of all kept: it lost a pair and a
bus where it gained a net, and doubled the time. What completed `ddr5-testbed` with its
constraints is pairs that take part in rip-up: fixed in the first pass, but in the passes on
the finer lattice a route like any other, which a net's search may cross at a price. The pair
is then taken out whole, and once the net is placed, routed again side by side in rip-up of
its own, moving other nets, but not pairs or that net, to be routed again after it; a pair
that finds no way goes back, for good. That is rip-up extended to pairs, not push-and-shove,
nor negotiation at the detailed level, which weren't needed.

**A net of many pads that can't be routed whole keeps what it reaches.** Routed as tracks, a
supply or ground of tens of pads fails whole in rip-up when one pad is shut in, and loses every
connection. A net of more than 16 terminals that rip-up can't route whole on the finer lattice
is searched for a terminal at a time instead, and keeps what it reaches; the next pass completes
it from that copper, rather than routing it all again. Only on the finer lattice: doing it from
the first pass, or for nets with planes too, changed the routing of the smaller boards and lost
`GB-LIVE32` and `nk3-nrf-mini` a net each.

**Zones are filled, not routed, and a net reaches its fill like a terminal.** `pcb::fill_zones`
fills a zone as KiCad's `ZONE_FILLER` does, on polyclip: the outline less what other nets need
around them, thermal reliefs to its own pads, parts too narrow taken away, islands removed. Its
fills are within 1% of KiCad 10's own on the curated boards, by the area only one of the two
covers ([results](benchmarks.md#pcb-zones-and-planes)). The router never sees fills as
copper: they are what is left after routing, filled again at the end around everything. What
it does see is where its nets' fills will be:

- Before routing, zones are filled around the fixed copper. The pads that a net's largest fill
  joins, on inner layers if it has fills there, are one terminal, which also holds places in
  that fill, near each of the net's other pads, where a via or a track's end fits. Routing the
  net is then routing stubs to its fill, by the same search and rip-up as any net, rather than
  tracks across the board. Placing the stubs first, as fixed copper, cost `nk3-nrf-mini`
  three nets, around its aQFN, which routing them as a net that others can rip up didn't.
- Inner-layer fills are planes, which other nets' tracks pay twice their length to cross,
  so as not to cut them in pieces.
- After routing, zones are filled again, and a net that its routes and fills leave in pieces
  has each piece joined to the largest by a stub, from any of its pads, tracks or vias, or
  by tracks, and is filled again, three times at most. Islands are kept until the very end,
  as a plane that no via reaches yet is still a plane to reach.

Fills are not obstacles to other nets' routing, as a zone of a lower priority isn't to one of
a higher in KiCad: the cost of a plane, and the repair after, take their place.

**On PCBs, placement and routing meet in a loop of windows, judged by routing.** M9's loop,
`kicad::place_and_route`, places once, routes once, and then each round places a window of
parts again, those on the nets that cost most beyond their half-perimeter in vias and detours,
and routes again only what that moved, with `pcb::eco::reroute`; a round is kept only if the
board it routes is better. Three things shaped it:

- *The judge is routing, not a placement estimate.* Weights and a map of what routing used
  steer the window's annealing, but the round is kept or dropped by its routed connections,
  starved thermal reliefs, vias and track. So the loop is never worse than the one pass by that
  measure, and estimates that mislead cost time, not quality.
- *Incremental routing keeps what nothing touched, byte for byte.* The copper kept becomes fixed
  copper of the design, nets it leaves complete are not routed at all, nor given escapes, and
  the lattice keeps the whole board's pitch, so a change of nothing keeps everything. Nets with
  zones lose only the stubs where parts were and are, not their whole plane's. That made a round
  cost seconds where the first routing of a large board costs minutes, and made engineering
  changes on routed boards the same mechanism.
- *Thermal reliefs are counted, and repaired.* Moved parts packed closer made routes crowd the
  pads of planes' nets, which KiCad reports when a relief has one spoke. The fill knows its
  spokes, so it counts such reliefs, the loop prefers boards with fewer, and a starved relief
  gets a place for a second spoke kept free while the nets across it route again.

Swapping equivalent pins is a separate step, by length, which the loop makes once after
placement if given groups: on the DRAM boards, length undid a careless DQ assignment but made
the designers' own worse, so a swap that pays has to be judged by routing too.

**No `unsafe`, at least initially.** The crate forbids it. If profiling later proves
it is needed, it gets isolated in a small audited module and the lint relaxed there.

## Open questions

1. **What is a "resource" on a PCB?** Settled at M5, at least for now: tile sides and
   tiles' vias in global routing, and nothing in detailed routing, where exact clearance
   queries make conflicts impossible and rip-up with a history of conflicts by tile resolves
   them. See [the decision](#decisions). M6's DRAM boards didn't need more: rip-up, with
   pairs taking part, completes them. Push-and-shove may yet change the detailed half.

2. **Static or dynamic dispatch for spaces?** Generics give the FPGA inner loop full
   inlining, at a cost in compile time and API friction. A generic core with a
   `dyn`-friendly wrapper for callers is the likely answer.

3. **Does search state belong to the engine or the space?** Settled, at least for now:
   the state belongs to the engine and the search, in flat arrays, and the space picks
   the mapping into them through `Slots` and `ResourceSlots`. `Grid` and `Graph` use
   `DenseSlots`, whose slot is the ID itself. A space with generated nodes, such as a
   PCB's, uses `InternedSlots`, which is a hash map. Searches reset theirs with generation
   counters, or by clearing the map. An engine keeps its resource mapping for the whole
   run and assigns a slot when a net first occupies a resource, between searches, so that
   pricing only reads the mapping and any number of searches can share it. On a miss, the
   resource is one that no net has occupied. Still open is whether the flat resource arrays
   should move to pages for the largest graphs, as they take 16 bytes for every resource
   up to the highest one occupied.

4. **How are global and detailed routing composed?** Settled at M5: as two spaces at
   different resolutions, where the global result restricts expansion in the detailed space
   to a corridor. The engine runs the global level, and the detailed level is searched net by
   net with rip-up, as [the decision](#decisions) explains.

5. **What does determinism cover across versions?** Same-version reproducibility is a
   requirement. Stability across versions would freeze every algorithmic improvement,
   so the current intent is that it is not promised, and that a seed and version
   together identify a result. Needs settling before 1.0.

6. **Do search costs need `f32` storage?** cadlab's grid search uses `f32`, possibly to
   save memory on large boards. We start with `f64` and only add a narrower or generic
   cost type if benchmarks justify it.
