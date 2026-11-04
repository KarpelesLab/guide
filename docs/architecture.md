# Architecture

**None of this is implemented yet.** It records the intended design so that early
code heads in a consistent direction, and it will change as implementation teaches
us things. Code sketches are illustrative, not an API.

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
   Geometry kernel: used by PCB-like domains only
   Validator: depends on the core model and domains, never on the engines
```

Dependencies point downward only. Engines never name a domain type. Domains that
have no geometry (FPGA) do not pay for the geometry kernel.

These start as modules in the single `guide` crate, with optional domains and
interchange formats behind Cargo features. We can split into a workspace later if
compile times or release cadence call for it.

## Core abstractions

### `RoutingSpace`

The central abstraction, and the one most likely to be wrong the first time. An
engine has to answer four questions, and a domain answers them however it likes:

```rust
pub trait RoutingSpace {
    /// A search state: a device wire, or (position, layer, direction) on a board.
    type Node: Copy + Eq + Hash;
    /// Something nets compete for.
    type Resource: Copy + Eq + Hash;

    /// Where can a route for this net go next, and at what base cost?
    fn expand(&self, net: NetId, from: Self::Node, out: &mut Vec<Step<Self::Node>>);

    /// A lower bound on the remaining cost, for A*. Must never overestimate.
    fn lower_bound(&self, net: NetId, from: Self::Node, to: Self::Node) -> Cost;

    /// Which resources does taking this step consume?
    fn resources(&self, net: NetId, from: Self::Node, to: Self::Node, out: &mut Vec<Self::Resource>);

    /// How many nets can share this resource before it counts as overused?
    fn capacity(&self, resource: Self::Resource) -> u32;
}
```

The two main implementations sit at opposite extremes, and the abstraction has to
serve both:

| | FPGA fabric | PCB |
| --- | --- | --- |
| Node | Index into a prebuilt graph | Point on a layer, generated lazily |
| Expansion | Read an adjacency list | Geometric query against the design rules |
| Resource | The wire itself, naturally discrete | Not naturally discrete (see open questions) |
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
interface, so that the two can be iterated together (roadmap M9).

## Decisions

Each of these can be revisited, but changing one should be a conscious act.

**Integer coordinates (`i32` database units; nanometres on PCBs).** Floating point
makes geometric predicates unreliable and results platform-dependent. `i32`
nanometres spans ±2.1 m, which is ample, and intermediate products fit in `i64`,
so predicates can be exact and stay fast. KiCad's internal representation is
similar, so conversion is lossless.

**Integer fixed-point costs.** Floating-point addition is non-associative, and
results can vary with evaluation order. PathFinder's multiplicative history costs
work fine in fixed point, with overflow handled by saturation.

**Determinism includes parallelism.** Results must not depend on thread count or
scheduling. Nets are routed in parallel in batches chosen so they cannot interact
(disjoint bounding boxes), and their results commit in a fixed order. Hash maps
with randomized iteration order are not allowed in any path that affects results.
Debugging, regression testing and benchmark comparison all rely on this, and
retrofitting it is close to impossible.

**The validator is independent.** A bug shared by router and checker is invisible,
so the validator shares the problem model only. On PCBs, we also cross-check against
KiCad's own DRC.

**Struct-of-arrays and indices, not pointers.** Large FPGA graphs make cache
behaviour a first-order concern. Nodes and nets are newtyped integer indices, which
also keeps the borrow checker out of graph algorithms.

**Minimal dependencies.** The crate's default build should need few or no external
crates and no external solver. Parsers and optional integrations go behind features.

**No `unsafe`, at least initially.** The crate forbids it. If profiling later proves
it is needed, it gets isolated in a small audited module and the lint relaxed there.

## Open questions

1. **What is a "resource" on a PCB?** Tracks conflict whenever they come within
   clearance of each other, which is a continuous relation with no natural discrete
   identity. Options: partition the board into cells that act as resources (natural
   for global routing, coarse for detailed), derive resources from conflicts
   dynamically (report "these nets conflict here", with congestion history kept in a
   spatial structure), or negotiate only at the global level and resolve conflicts
   with push-and-shove in detailed routing. We expect to prototype at M5, and
   `resources`/`capacity` may need replacing with a conflict query.

2. **Static or dynamic dispatch for spaces?** Generics give the FPGA inner loop full
   inlining, at a cost in compile time and API friction. A generic core with a
   `dyn`-friendly wrapper for callers is the likely answer.

3. **Does search state belong to the engine or the space?** FPGA search state is best
   a flat array indexed by node, with generation counters for fast reset. PCB nodes
   are not dense, so they need a hash map. The space probably has to pick its own
   node-to-slot mapping.

4. **How are global and detailed routing composed?** Possibly as two spaces at
   different resolutions, where a global result restricts expansion in the detailed
   space. If so, one engine handles both levels. This needs validation at M5.

5. **What does determinism cover across versions?** Same-version reproducibility is a
   requirement. Stability across versions would freeze every algorithmic improvement,
   so the current intent is that it is not promised, and that a seed and version
   together identify a result. Needs settling before 1.0.
