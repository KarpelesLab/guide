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
`mapf` for multi-agent path finding, negotiated over safe intervals or through a time-expanded
space that the engine routes agents through unchanged, with a reader for the MovingAI
benchmarks, and `asic` for ASIC global routing on the ISPD contests' designs.

The `guide` command-line tool, `src/bin/guide`, sits on top of all of it, behind the `cli`
feature, which enables every domain. It uses only the public API, as any other consumer would,
so it is also a check that the API is enough to run each domain from its files to its files.
It has no dependency either: a small table-driven argument parser gives each command its
options and its `--help`. Commands follow one pattern, `guide <domain> <action> [options]
<files>`, with the same options for the same things everywhere (`-o`, `--json`, `--seed`,
`--threads`, `--time-limit`, `--work-budget`, `-q`, `-v`) and the same exit status: 0 for a
complete and legal result, 1 for an incomplete or illegal one, 2 for an error.

**Run control.** Every long run can be stopped and heard from the same way, through
`control::RunControl`: a deadline, a cancel flag, a work budget and a progress callback, taken
by the entry points named `…_with` (`place::anneal_with`, `place_analytic_with`,
`place_incremental_with`, `fpga::place_design_with`, `refine_placement_with`,
`place_route_loop_with`, `pcb::place::place_components_with`, `refine_components_with`,
`kicad::place_and_route_with`, `asic::route_design_with`, `mapf::solve_with`), each the plain
entry point when the control is the default, bit for bit. A run stopped by any of them returns
its best legal result so far and says why, as a `control::Stopped`. The work budget is counted
in the run's own work, annealing moves and nodes expanded, together where a flow does both,
and is the reproducible one; a deadline and the flag depend on the moment. One-threaded parts
look at the clock and the flag themselves, the annealer every thousand moves; the engine reads
a single flag, so a run with a deadline that negotiates has a scoped thread watch the clock and
the caller's flag and set the engine's, as the tool's timer did. PCB routing keeps its own
`pcb::RunControl`, whose progress knows its phases and nets, and whose `Stopped` is the
crate's; the flows that route PCBs hand it their deadline, flag and what is left of their
budget. The placers' options keep their own cancel flag and move budget, now with a deadline,
and the engine its cancel flag and expansion budget, for callers that step them. `fpga route`
still stops the engine through its cancel flag set from a timer thread, and prints each
iteration with `-v`.

There is no Ctrl-C handler. std has no signal API; on Unix one would call `signal` or
`sigaction` through `extern "C"`, which is `unsafe` and the crate forbids it, and on Windows
`SetConsoleCtrlHandler`, likewise; reading standard input to its end is no substitute, since
Ctrl-C doesn't close it. Doing it without either needs a dependency such as `ctrlc`, which the
tool doesn't take. So Ctrl-C ends a run at once, without writing anything, and the tool offers
`--time-limit` to stop early and keep the result, and `-v`'s progress lines to see how far a
run has got.

The examples in `examples/` stay what they were: research harnesses with every tuning knob,
printing `key=value` lines that the scripts in `scripts/` parse. The tool exposes the settings
a user needs, with the library's defaults.

## Target consumers

Two existing crates are expected to adopt `guide`, and they constrain its design:

- [reticle](https://github.com/KarpelesLab/reticle), an HDL compiler with its own
  FPGA placer and router. Its contributing rules forbid third-party crates, and
  all of its current dependencies are optional.
- [cadlab](https://github.com/KarpelesLab/cadlab), a headless PCB CAD tool with its
  own autorouter, built on [polyclip](https://github.com/KarpelesLab/polyclip).

Both currently declare Rust 1.89 and will move to `guide`'s 1.95 before adopting it.

On reticle's own designs and devices, `guide` already routes everything reticle's router
does, legally by reticle's own checks down to the decoded bitstream, in less time overall and
with fewer pips: see [routing reticle's designs](benchmarks.md#routing-reticles-designs), which
also lists what reticle would need to expose for `guide` to be dropped into its flow.

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
steps a temperature at a time with a budget, a deadline and a cancel flag, as the routing
engine steps iterations, and every placer takes a `control::RunControl` through its `_with`
entry point, and the interface is `CongestionEstimate`, an overflow for a rectangle, which a
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
  reaches them, in slots kept for as long as the net is searched for, searched one net at a
  time with `search::Search` (not the engine), inside the net's corridor. It has no resources at all:
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

**A net's searches share one tree, and a connection is looked at from both ends.** Most of
the time of detailed routing went into searches that could not succeed, and into searching
again for what was already found. A terminal that other nets' routes shut in was found out by
a search from the tree through the whole corridor, and then by a search of up to 50,000 nodes
from it over the board; and the search that may cross routes then went everywhere it could
reach before it crossed the one route in its way, since a crossing costs many times a path's
length, and the bound, which is a length, has nothing to say about it. Each of the three
routed the whole net from nothing. Now ([results](benchmarks.md#pcb-routing-searched-from-both-ends)):

- *One tree.* A net is a `NetRun`, whose tree the search in the corridor, the one over the
  board and the one that may cross routes grow in turn, each from what the last connected.
- *Looks.* A connection is searched for in steps: a glance of 256 nodes from the terminals,
  a search of 8,000 from the tree, a look of 2,000 from the terminals, a search of 64,000, a
  look of 50,000, and only then the search with the net's budget. A look that ends shut in
  ends the search. One that reaches the tree only says that the search from the tree will:
  the path is still that search's, to the nearest terminal of all. Taking the look's own path,
  to the first terminal, lost connections on `nk3-nrf-mini` and `ddr5-testbed`.
- *Crossing from the side that is shut in.* Where routes may be crossed, the search is made
  from a terminal that a look found shut in, towards the tree: the first route it crosses is
  one that shuts it in. It ends at a node that stands for the whole tree, with a bound to the
  nearest of the tree's runs, kept in a grid. It is kept within 24 pitches of the terminal and
  the tree at first, and goes anywhere only if that finds nothing: without, `ddr5-testbed`
  took 60 s in place of 36 and lost three connections. A terminal shut in by what may
  not be crossed is found out the same way, in place of a search from the tree that runs
  through its budget.
- *One lattice away from pads.* A pad's lattice goes on only within the 8 pitches of the pad
  from which routes move onto it. Anywhere else a route is on the lattice of the net's first
  pad, with a step of less than a pitch between the two. A net of many pads used to be
  searched for over the board once for each pad's lattice, by every search that found no path.
- *Bounds kept.* Every node of the tree is a source of every search from it, and asked for its
  bound to every terminal left, each time. The slots of a net's nodes are now kept from one
  search to the next, so `Search::reuse_bounds` applies, as it does in the engine.

Weighting the bound was tried again, from 1.2 to 2, and from 2 to 4 for the searches that
cross routes: a search that finds a path expands a quarter of the nodes at 1.5, but nets then
rip each other up more often, and the boards took longer between them than without. A via's cost in
the bound, also tried again, saved a tenth of the vias over the boards and lost three
connections on `ddr5-testbed`. Giving up on the corridor after the looks, in place of
searching it whole, doubled the time of the boards that need the finer pass. Results are as
chaotic as before: whether the first pass leaves a net for the finer one decides most of a
board's time.

**What rip-up takes out, what a stopped run keeps, and what a net is routed at.** Measured
after the above ([results](benchmarks.md#pcb-routing-after-the-release-candidate)):

- *Branches of a net of very many pads.* In the passes on the finer lattice, a net of 32
  terminals or more with no plane, crossed by another, loses only the copper that the other's
  route is in the way of, cut back to the nearest joint (`NetCopper::free_in`, `prune`), and
  is completed from the pieces that leaves, each a terminal (`NetCopper::pieces`). It may lose
  branches eight times in a pass before it is taken out whole. Everything narrower lost
  connections: at 8 terminals, `nrfmicro`'s ground of 24 pads, cut sixteen times and then
  with no way back, took 15 connections with it; nets with planes, whose routes are stubs
  that come back after the nets that crossed them, lost one to three on `ddr5-testbed` when
  cut, in the first pass or after, counted as rip-ups or not, with dead ends kept or pruned.
  Taking a net out whole is also what reorders it behind the nets that needed its room.
- *The best routes so far.* Before a route takes others out, the routes of every net are
  kept if none kept before made more connections, and a run that is stopped goes back to
  them if it has fewer: more work never leaves fewer connections.
  `RunControl::fill_on_stop`, off, has a run stopped by the clock or the flag return without
  filling its zones, which took up to 19 s past the deadline.
- *Neck-down.* A net whose width does not leave one of its pads, straight from its centre in
  any of the eight directions, is routed at the widest width that does, not under the rules'
  least, as one width for the net (`RouteOptions::neck_down`). A width only near the pads
  would need a second width in the search.
- *A pair's ends.* A pair whose nets have two pads each at an end is tried between its four
  best choices of ends, in place of the farthest alone.
- *Meanders* go down to the lowest height a meander can be, not to an eighth of the highest.
- *Fixed zones in parts.* A fill given as fixed copper is kept as parts of 48 vertices at
  most, each a nanometre larger all round. No board measured shows a gain from it.

**What a run with a budget does otherwise, and what a pour is.** Measured after the above
([results](benchmarks.md#pcb-routing-with-a-budget)):

- *A run with a work budget or a deadline routes for the most connections soonest.* In its
  first pass a net that finds no way in its corridor waits until every net has had its turn
  before it is searched for over the board and takes others out (`Wait`), and a net with a
  plane and 32 terminals or more that is in another's way loses its stubs in the way, not
  all of them, once for every four of its terminals (`loses_branches`). A board of six
  layers cut at 60 s went from 659 connections open to 399. A run with neither is as it
  was, and differs from one with a budget that is never spent: on the crowded curated
  boards the wait cost a connection and a quarter more vias on one, and the stubs a
  connection on another, as they had before. The two aims are not one order of work.
- *A pour is copper.* A zone of a net of four pads or fewer, 32 pitches of the lattice wide
  or less, is not a plane unless it says so: its fill is fixed copper, as
  `CopperZone::plane` false makes any. As planes, such pours joined their pads, so that the
  net had nothing to route, were cut by the routes of the pins beside them, and left their
  pads apart with no way back. By the number of pads alone, a ground of four pads under a
  whole board was copper to go around.
- *A net with zones left in pieces has a cause.* What the joining of zones' pieces could
  not join was searched for, and is `Cause::NoPath` or `Cause::Enclosed`: `Cause::Unknown`
  read as a net that the router took for routed.
- *A host's fill is measured to by its parts*, as a fixed zone is in the world: what is not
  within a clearance of a part of 48 vertices, a nanometre larger all round, is not within
  it of the fill, and only the rest is measured against the fill, which keeps the result
  exact. 9 s of a budget of 60 on the largest board, before anything was routed.
- *`RouteOptions::fills`*, off, for a host that fills again: the fill around everything is
  not made, and the connections are counted on the last fill of the joining of zones'
  pieces, which differs from it by its islands alone, which join nothing.
- *`RouteOptions::fanout_via`*: a trapped pad that its net's via fits nowhere near gets its
  dog-bone with a smaller one.

Tried and not kept there: a pour's net as tracks; any zone of a net of few pads as a pour;
dog-bones planned for every ball inside an area array, which made two boards with BGAs slower
to no gain, as the ways between the vias are no more on the lattice than those between the
balls.

Tried and not kept: a smaller budget for the searches that cross nothing in the passes on
the finer lattice (an eighth made `stm32-2l` slower, 9 to 13 s, a thirty-second faster, 9 to
6 s: no trend); the last fill of zones' joining kept as the result's, which only applies
when every zone keeps its islands. What still costs most on a board with large zones is
filling them: once to plan, once for each round of joining their pieces, and once at the
end, 10 to 17 s each on `lumenpnp-mobo`.

**A budget is shared out between routing and what comes after, and gates have lattices of
their own.** Measured after the above ([results](benchmarks.md#pcb-routing-in-parts)):

- *Routing has a share of a deadline or a work budget, and cleanup and the joining of zones'
  pieces the rest.* A run that its budget stopped in the middle of rip-up went back to its
  best routes and returned: the stubs that join a plane's pieces, tens of connections for
  seconds of work, were never made, and a host saw every pad of a cut plane open. Routing
  now ends at three quarters of the time left when it begins, cleanup has a third of what
  is left then, and joining the rest, where it starts no fill that it would not see the
  end of. A run whose routing ended so says that it was stopped. `RouteOptions::join` turns
  the joining off, and its share with it, for a host that fills its zones itself and
  routes again what its own fill leaves apart.
- *Every phase before routing ends with the deadline.* The fills that tell the planes and
  that nets are planned on are given up between two zones, the places for a plane's stubs
  are not looked for to the last pad, and the fill made to tell the planes by their
  coverage is the one that nets are planned on, where nothing was added to the board in
  between: one fill fewer before a run on a routed board.
- *A fill of many edges is asked about by its parts.* A via in the middle of a host's fill
  is in one of its parts, of a few edges, and so is within any clearance of the fill: only
  what is within two nanometres of a clearance of a part is measured against the fill,
  every edge of it, which keeps the answer exact. A piece of fill says where a point is the
  same way, and a piece that copper of its net has its middle in touches that copper,
  with no parts made.
- *The nodes of a net's searches are numbered by block of its lattice.* A hash map from
  node to number and another to the verdicts on its steps were a quarter of the time of a
  search, a cache miss each, twice for every step: the nodes of the net's own lattice are
  now numbered in blocks of 8 by 8 on a layer, a node's neighbours beside it, and the
  verdicts are kept by number. The same nodes, the same order, the same routes.
- *Gates.* A passage between two pads or vias that is as wide as a track and its
  clearances, or less than a pitch more, has an anchor in its middle with a lattice of its
  own, which routes move onto near it as they do onto a pad's; where a gate between two
  items side by side meets one between two items one above the other, the anchor is where
  their lines cross. The passes on the finer lattice of a run with a budget use them. The
  first pass does not: with them in every search, the balls of `glasgow-revC3`'s array were no longer shut in,
  and the board ended with more connections open, not fewer, as each of them then took
  others' routes out for a channel of one track, and every node near an array cost several
  steps more. And a run with no budget does not: `ddr5-testbed`, with its pairs routed
  first, lost a connection of 255 to them.
- *What is left open says more.* `Routed::tried` has the nets that had a search, a pad
  that is shut in is said to be after a run that was stopped too, and a result with no fill
  of its own is counted on the fill that routing planned on, so that a stopped run of a
  host that fills its zones itself does not list every net with a zone as unrouted.

Tried and not kept there: a weight on the bound of a net's searches, 1.5, which expanded
more nodes, not fewer, as nodes were queued again, and with each node expanded once at most,
fewer nodes of which more were new, for the same time and more vias; rip-up given up for a
pass once it made no more connections for a million nodes, which took `glasgow-revD1` from
344 connections open to 195 with a budget of 60 s and every other board the other way,
`buspirate5-rev10` from 16 to 61; and a first pass with no rip-up until every net had had
its turn, for all nets or for those that wait, which left more open on six or seven boards of nine.

**A run with a budget plans the ways out of what a net-by-net search shuts in, and moves a
net aside before it takes one out.** Measured after the above
([results](benchmarks.md#pcb-routing-with-ways-out-planned)):

- *Side by side out of a row.* A net too wide to leave a pad was necked to the width that
  leaves that pad, past the pads beside it. Two such tracks out of two pads side by side are
  then too close to each other, by ten micrometres on a 0.4 mm QFN with 0.19 mm of clearance:
  every other pad of a row got out, and the rest were shut in by their neighbours' tracks,
  whichever were routed first. A pad with pads of other nets beside it is necked to the
  width at which a track leaves it the same way as one leaves each of them, clear of it,
  and a net with a pad in such a row is on that pad's lattice, so that its track needs no
  step sideways where it has no room for one.
- *Balls deep in an area array.* Between two balls that `n` tracks pass, `n + 1` rings get
  out on their layer and no ball further in does. Such a ball is not trapped while nothing
  is routed, so it had no escape, and where its net's own via fits nowhere beside it, the
  search, with one via a net, had no way to take it to another layer. It gets its dog-bone
  with the smaller via, on the side of the array's middle where it can, the balls furthest
  from the middle first: a via between the balls further out closes the ways that those
  balls get out by, which is what a dog-bone the same way for every ball did when it was
  tried. Only for the balls that deep, and only where the net's own via does not fit.
- *A net in the way is moved aside before it is taken out.* A route that has to cross
  others' used to take each of them out whole, to be routed again from nothing when its turn
  came: searched for over its whole length for the few pitches of it that were in the way,
  and back by another way altogether, in the way of other nets somewhere else. Now the
  nets crossed are first cut where the route crosses them and mended around it by a search
  that crosses nothing, of 20,000 nodes at most, all of them or none: if every one is
  mended, a connection is made and none is lost, and if one can't be, everything is put
  back. Then, as before, they are taken out, but cut and mended too: a net keeps what of
  its copper the route leaves free, has its turn next, mends itself from the ends at the
  cut without crossing the net that cut it, crossing others if it has to, and is taken out
  whole only after eight cuts in a pass, or if it can't be mended.
- *A pass never ends with fewer connections than it had.* The routes that made the most
  connections are kept through a pass, as they were for a run that is stopped, and a pass
  that ends below them goes back to them.
- *What a host is told.* `Routed::failed` has the open nets whose own search found no way:
  the others lost their routes to another net and have room on the board as it is returned.

A run with no budget has none of these: the curated boards are as they were, to the via
and the millimetre. With the necking alone in such a run, the narrower tracks found other
ways on two of them, and `nk3-nrf-mini` ended a connection short.

Tried and not kept there: a fan for each row of fine pitch, a track out of each pad and
then at 45° away from the middle of the row, each a lattice step deeper than the one outside
it, as fixed copper that routes go on from, which made no more connections over the boards
and orders measured, cost `corne-cherry` 240 vias, and left two of cadlab's generated
boards, which complete, five and six connections short, the fixed tracks in the way of the
routes round the part; rip-up of the first pass given up once it has made no more
connections for half the work that its most took, which left the nets still out to the
passes on the finer lattice, where they cost more nodes for fewer connections, 542
connections open on twelve boards for 439; cutting and mending with no limit on the search
for a way round, which made each mend a search of the whole corridor, 300,000 nodes and
more; and mending that may cross the net that cut it, where two nets then took each
other's place in turn until each had been cut its eight times.

**What a run does before it searches is done once, and a search asks the index once for a
node.** Measured after the above ([results](benchmarks.md#pcb-routing-with-ways-out-planned)):

- *One query for the steps from a node.* A search asked the index of shapes what was near
  each of the eight steps from a node, three queries for a node with what the cache of
  steps knew, and the walk down the tree was a third of a search. A node with more than
  one step to judge asks once what is near all of them, and each step is measured against
  those of the entries that its own query would have found, in the same order
  (`World::near`, `World::track_among`): the same verdicts, so the same nodes and routes.
- *The fixed items packed.* A board's pads, zones and edges went into the index one at a
  time; they are packed as a tree built of all of them once the last is in
  (`SpatialIndex::repack`), under the same identifiers, and routes go in after as before.
- *The parts of a fill worked out once.* A fill of thousands of edges is measured to by
  its parts of 48 vertices, and was cut into them again for each world a run builds, each
  fill of the zones that starts from a host's, and each piece of a plane that a pad asks
  about: by halves, with a clip of the whole at each. The parts of the last shapes cut are
  kept, a million vertices of them by thread, by what they were cut from (`Shape::parts`).
- *A host's fill cut in windows.* Where copper came closer to a host's fill than its
  clearance, the whole fill was cut and opened by the zone's minimum width. Nothing
  changes further from the copper than twice the radius of that opening, so the fill is
  cut and opened in a window of that round each piece of copper, and is as it was given
  outside the windows, to the vertex.
- *Global routing that ends when it gains nothing*, in a run with a budget: after three
  iterations in a row that take less than a fiftieth off the least overuse so far, with
  the best iteration's routes. A run with no budget has its thirty iterations.
- *Tight before fewer vias*, in a run with a budget. Cleanup searched each net for a
  route with fewer vias and then pulled every net tight, and its share of the time ended
  in the searches on a board that a budget cuts: the tracks of every net were left as
  rip-up and mending had them. Pulling tight takes no search, so it comes first, and
  again for a net that a search routes anew.
- *A pad is said to be shut in at the width its net was routed at.* The flood that looks
  for the cause of an open connection used the net's own width, at which a net necked for
  a row of fine pitch never leaves its pad: every open pad of such a row was said to be
  shut in by the two pads beside it.

None of these changes a route of a run with no budget: the curated boards are as they
were, to the via and the millimetre and the node, in a sixth less time. A run with a
budget has other corridors where its global routing ends sooner, and other tracks where
they were not pulled tight.

Tried and not kept there, all for the pads of rows of fine pitch that stay shut in on
boards of two layers, and all measured over three orders of the nets, as one run of a board
moves by ten connections from one order to the next: *lanes*, the ways out of such pads
priced for other nets' tracks and vias for as far as a via and its clearances take, which
made routes leave a row straight, as a router with a planned fan-out has them, and left as
many pads open on the board it was made for and more on the boards of four and six layers,
whose rows have their vias at the ends of their pads; the same with vias of other nets
forbidden in the lanes that lead away from a part, and with other nets' tracks forbidden in
the first two pitches of a lane; a history of conflicts by cell of two pitches in place of
by tile of sixteen; the pitch of the lattice taken from the necked widths, half the pitch
of the row; the nets of such rows routed first; sixteen rip-ups of a net before it is
locked, for four; and the passes on the finer lattice ended once they had gone on for half
the nodes of the run before them with no connection more.

**A run with a budget joins what its routes cut off the planned fills before it ends, and
plans the ways out of a row of fine pitch for the whole row if asked.** Measured after the
above ([results](benchmarks.md#pcb-routing-with-a-second-wait)):

- *Rows.* A row is fine if a via in the way out of one of its pads is in the way out of the
  pads beside it: 0.4 mm of pitch for a via of 0.6 mm, a track of 0.2 mm and as much of
  clearance. Two tracks side by side out of two such pads leave neither a place for a via,
  which is why a price on the ways out did not settle such rows, nor a wall: the choice is
  the row's. Rows are told from the pads alone, with no footprint to say so: pads with no
  hole on one layer, of one size, in line at one pitch along x, y or a diagonal, four at
  least, and not in rows more than one way, as the balls of an area array are. Each pad of
  a net that is routed then has ways to choose from: none; a straight track out of either
  end, past the pad by what a via beside it takes, on a side where it has room to go on;
  or such a track with a via at its end, at up to three depths where the world has room
  for one, looked for in steps of an eighth of the lattice's pitch, as the room between a
  row and the exposed pad under its part is a few tens of micrometres long. A way's cost is
  its stub's length, a via's cost, and how far its end is from the nearest other terminal
  of the net, a via more if that is on no layer the stub's end is on, and the row's length
  more for a plain track if it is behind the row; no way costs what crossing a route
  does, unless another terminal is within 1.5 mm on the pad's layer. Two ways of two pads
  clash if the via of one is too close to the other's via or in the other's lane, which
  for a plain track goes on past its stub, and two plain tracks side by side on one side
  cost 3 mm. The ways of a row are chosen together, at the least cost with no clash,
  exactly, by the ways of the last three pads. On a QFN with an exposed pad that is every
  other pad to a via under the part and the others straight out, on a part with none
  vias on both sides by turns, and the pads of a net with a plane have their via to it
  in the row's plan.
- *Stubs are routes.* What is chosen is copper of its net in the world before the first
  net is searched for, as a net routed in part has its copper: each stub a piece with its
  pad, its end and its via as places to go on from. A net's first search with stubs is its
  first search all the same, in its corridor, and waits like any other. A net routed
  whole loses the stubs it did without, a net in the way of another is cut or taken out
  with its stubs, and a net left open has none in the result. A stub's end is kept
  through the prunes of a cut net (`NetCopper::keep`): a net of many pads, or with a
  plane, that is cut anywhere is cut back to what joins something, and lost every stub
  of its pads in rows the first time, a supply's as a ground's.
- *Pours.* A net is planned on its zones' fill as it is before routing, and the pads that
  the fill joins need no route. On an outer layer other nets' routes go through that fill:
  on a board of two layers with a ground pour, a host that filled again found some sixty
  pads off their pour after one run, twice the connections that routing itself left open.
  The planned fills are kept as cells of a grid as fine as the lattice, by scan line, with
  the relief of each pad they join. Once routing is over, the cells within the fill's
  clearance and half its least width of another net's routes are taken out, what is left
  is told into its pieces, joined through the net's pads, tracks and vias, and each piece
  with a pad or a stub of the net on it that is not the largest is a terminal to join to
  the largest: by a search that moves nothing, to places in the largest piece near it
  that are kept as places of the net from then on. Three times over at most, in cleanup's
  share of the time and three hundredths of a work budget. What the grid leaves apart
  before anything is routed is its own coarseness, and is never told to be cut off; the
  grid errs on the side of a cut, and a stub.

A run with no budget has none of these, and the curated boards are as they were. The
rows are planned only if asked (`RouteOptions::row_fanout`, off by default): through
cadlab's adapter at 60 s over three orders of the nets, with the wait below,
`corne-cherry` has 99 connections open with them for 106, `glasgow-revC3` 127 for 113 and
`buspirate5-rev10` 14 for 11, and the eleven boards without `glasgow-revD1` 367 for 358.
The pours are joined by default (`RouteOptions::pour_cuts`): 358 for 381 the same way,
`corne-cherry` 106 for 118 and `tinytapeout-demo` 22 for 28.

Tried and not kept there, each over six orders of the nets of `corne-cherry` with a budget
of 40 million nodes, by the connections that the run itself counts open, 163 as kept
before the stubs' ends were: the stubs of a row locked until their nets' first searches,
so that no search of the first sweep crosses one, 205; a net taken out whole given its
stubs back, where the world still had room for them, 194, and 222 with both; and the pads
cut off their fill joined to it again while routes are still moved, every 64 routes
placed, with the right to move what is in the way or without: a ground whose pour is in a
dozen pieces then has a dozen stubs to make across the tracks that cut it, each in the
way of the nets still to settle, and one order had 41 to 46 connections open for 23,
and more as its host counted them, not fewer. A plain stub for each pad that its net's
fill joins, in any row, fine or not, to keep the fill's way to the pad open, changed no
route of the four boards it was measured on. Keeping the stubs' ends is not a gain on
every board either: `corne-cherry` has 191 for 163 with it, and `buspirate5-rev10` 13 for
36.

**A net that waited is searched for with nothing moved before it crosses anything, and
keeps what that reaches.** Measured after the above
([results](benchmarks.md#pcb-routing-with-a-second-wait)):

- *A second wait.* In the first pass of a run with a budget, a net that found no way in
  its corridor waited for every net to have had its turn, and then had all of its
  searches at once: over the board with nothing moved, then across other nets' routes,
  whole and then a terminal at a time. A search with nothing moved ends soon where the
  net is shut in, and one that crosses routes costs up to its limit of nodes whether it
  finds a way or not. On `glasgow-revD1`, with 561 nets, 114 nets were waiting after 9 s
  of a run through cadlab's adapter, with nothing of their 436 connections, and 94 of
  them, with 313 connections, still when the run was cut at 27 s: the few that had had
  their turn had spent the time moving each other. cadlab's second run, on the routed
  board, where nothing moves, then made 110 to 160 of those connections in as many
  seconds, which is why its first run could not be given more of the budget. Now a net
  that comes back is searched for over the board with nothing moved, and if that finds
  no way it waits once more (`Wait::Cross`) before it may cross anything. With
  `RouteOptions::keep_partial`, a net of more than two terminals keeps what that search
  reached, as a net routed in part, and its turn to cross other nets' routes completes
  it from there, or leaves it what it has. The same run has 1692 connections made after
  15 s and 1757 after 22 s where it had 1612 and 1620.
- *Side by side in an area.* The width at which two tracks leave two pads side by side
  was told by the clearance between their nets, and the clearance of a rule area that
  both tracks are in was not looked at: in the courtyard of a QFN that a board's rules
  give 0.18 mm, the supply pins of classes with 0.25 mm kept the width that left each
  pad alone, 0.24 mm, at which two do not leave two, as no width the rules allow is
  0.25 mm apart from its neighbour at a pitch of 0.4 mm. An area's clearance now takes
  the place of the nets' there, as it does for every piece of track that is laid.

A run with no budget has neither, and the curated boards are as they were.

Tried and not kept there. Through the host, on an uncommitted
change of cadlab's adapter: runs of `eco::reroute_with` after the two runs it makes, each
with the routed copper within 1.5 mm of the ends of what is still open taken out and
routed again, in another order of the nets each time, and a run on the board filled again
after each. `nrfmicro` had 12 connections open over three orders for 18 and
`buspirate5-rev10` 9 for 11; `lumenpnp-mobo` and `tinytapeout-demo` gained nothing, a run
that took out the copper round five to eleven open pads leaving 50 to 70 open where it
was cut, as a net with no zone that touches an area is taken out whole. At 3 mm and
4.5 mm no run of any board gained. And of guide's own, for the pads that routes cut off
their fill for good: a pad with a pad of another net close beside it made a terminal of
its own though the fill joins it, which most pads of a dense board are, 677 connections
open on `glasgow-revC3` for 113; and the pads of rows alone given a stub through a via to
the fill of another layer before anything is routed, as fixed copper, 343 for 337 on
eleven boards. What `lumenpnp-mobo` had open was neither: its host did not ask the first
run for the ground that its own fill joined, and asked, the run leaves 1 open over three
orders for 25.

**What rip-up leaves open in a run with a budget is settled by chains that are kept only
if the board has more for them.** Measured after the above
([results](benchmarks.md#pcb-routing-settled)). Rip-up takes a net out for another and
routes it again when its turn comes, on a board that has changed by then: a pass of
`corne-cherry` had 395 of its 432 connections after 4 million nodes, and 396 at best
23 million nodes and some hundreds of nets taken out later.

- *A pass ends where it has stopped gaining.* A pass of a run that settles ends once
  every net has had a turn since its routes made the most connections they have made,
  and 2 million nodes have gone since: in the first pass, as many again as it took to
  get there, if that is more. It then goes back to those routes, as a pass that ends
  with fewer always did.
- *Chains.* For each connection still open, a route is looked for as rip-up looks for
  it: with nothing moved, and then across other nets' routes at rip-up's price, four
  nets at most. The nets crossed are cut where it crosses them, keep what is free of it,
  and are joined up again at once, in turn, each across further nets if it finds no way
  round, three nets deep and twelve nets in all; a net that the chain has routed is not
  crossed again in it. If every net that was cut is whole again, and the routes then
  leave less open, they are kept. If not, every net of the chain has the copper back
  that it had, and the next try for the connection, of four, may not cross the net that
  was not joined up again. So no chain leaves less than it found, and the order in which
  nets come back, which is what a pass of rip-up is at the mercy of, is the chain's own.
- *What is open is what a host will count.* The routes of a chain go through the fills
  that nets are planned on, as any route does, and a connection made by cutting two pads
  off their pour is a loss. So what a chain is judged by is the connections that the
  routes leave open and the pieces of fill with pads on them that are cut off their
  net's fill, as the grid of `RouteOptions::pour_cuts` tells them. On `corne-cherry` the
  run then says 34 open where its host finds 45, for 23 and 40 without. The pieces
  themselves are joined by the stubs after routing, as before: until 0.0.11 a net with
  such pieces had chains of its own first, with each piece as a terminal to join to its
  fill again across what is in the way, which made no more of them (232 open for 235 on
  six boards at 40 million nodes without those chains) and went on for 14 s of the 42 s
  that `lumenpnp-mobo` had, with every one of its nets routed, so that its host's second
  run was not started.

`RouteOptions::settle`, on by default; `Stats::settled` says how many chains were tried
and kept and what they made, and `Phase::Settle` is told. A run with neither a budget nor
a deadline settles nothing and ends no pass early, and the curated boards are as they
were. With a budget of 40 million nodes, over three orders of the nets, the nine boards
of cadlab's corpus that a budget cuts have 853 connections open for 892: `corne-cherry`
140 for 169, `cynthion` 63 for 67, `glasgow-revC3` 107 for 111, `buspirate5-rev10` 11 for
13, and the others the same.

Tried and not kept: chains kept with a net left cut where the routes made more in all,
which went on to a dozen nets and were undone; chains half as long and half as deep at
three times the price of a crossing; a window after the chains, the nets of few pads
beside the open pad taken out whole and routed again after it; a fixed share of the
budget for the chains with no end to the passes; the count without the pours; and, once
the chains have made what they can, chains twice and four times as long, deep and wide
for what is left, which made nothing on four boards.

**The layers that a net joins at a point are those of all its places there.** A track's
end and a via at the same point are two places of a net, as a stub to a plane has them once
it is fixed copper of a later run. Asked from the track's layer, the first of the two said
that nothing joins the layers there; asked from another, the second said that the via does.
A search only steps through from where it is free to, but a path found from its far end is
laid the other way round, and its change of layers was then taken for one that needs a via:
a via on a via, which a host's check took out with the piece it was on. The places at a
point are merged by the layers they share, and the answer is the same from either side
(`NetCtx::joins`).

**Differential pairs are routed side by side last, in place of their nets' routes.** Routing
pairs first, before anything else, as fixed copper, coupled `udb-s`'s USB pair a little better,
60% of its length side by side in place of 50%, but cost other nets their room: `GB-LIVE32`
lost a net to its USB pair, and routing it again with the pair as two nets doubled its time.
Routed last, each pair is taken out, routed side by side
where it fits, and kept only if it connects what its two routes did, so no other net changes
at all, and a pair that doesn't fit stays as two nets. Meanders come after that, for the same
reason ([results](benchmarks.md#pcb-differential-pairs-and-lengths)). On the DRAM boards,
though, routed last, no pair finds room through the buses around the BGA, even with the routes
in its way ripped up and routed again, which was tried; so `RouteOptions::pairs_first` routes them
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
   together identify a result. Needs settling before 1.0: the
   [draft stability policy](stability.md#across-versions-the-open-question) sets out four
   options, from no promise to versioned behaviour, with their trade-offs.

6. **Do search costs need `f32` storage?** cadlab's grid search uses `f32`, possibly to
   save memory on large boards. We start with `f64` and only add a narrower or generic
   cost type if benchmarks justify it.
