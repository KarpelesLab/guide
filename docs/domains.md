# Domains

What each target domain needs, how it maps onto the core, and what stays
domain-specific. PCB, FPGA and multi-agent path finding are implemented; see the
[roadmap](../ROADMAP.md) for ordering.

## Summary

| | PCB | FPGA | ASIC | Multi-agent paths | Qubit mapping |
| --- | --- | --- | --- | --- | --- |
| Terminals | Pads | Logic and I/O pins | Cell pins | Start and goal | Interacting qubits |
| Routing space | Continuous, per layer | Fixed graph | Tracks on layers | Grid or roadmap, plus time | Coupling graph, plus time |
| Contended resource | Copper clearance | Wires, muxes | Tracks | Cells at a time step | Physical qubits at a time step |
| Placement | Components | Logic to sites | Standard cells, macros | Task assignment | Initial qubit layout |
| Fixed items | Connectors, holes | I/O, hard blocks | Macros, pads | Obstacles | Device topology |
| Main objective | Completion, DRC-clean, signal integrity | Critical-path delay | Timing, DRC-clean, wirelength | Makespan, total cost | Gate count, fidelity |
| Roadmap priority | Primary | Primary | Global routing implemented (`asic`) | Implemented (`mapf`) | Exploratory |

## PCB

**Input.** Board outline and layer stack, components with footprints, nets, design
rules, and any pre-existing tracks. Components may be fixed or movable.

**Routing requirements.**

- Multiple layers with per-layer preferred direction and width.
- Through, blind, buried and micro vias, each with a drill, annular ring and cost.
- Clearance rules varying by net class, layer and region; keepout areas.
- Differential pairs, with coupling and permitted gaps.
- Length and skew matching (meanders), including matched groups such as DDR byte lanes.
- Net topology rules (daisy chain, star, T) and per-net width by current or impedance.
- Fanout and escape routing under BGAs and fine-pitch parts.
- Planes and copper zones, with thermal reliefs.
- 45° (octilinear) routing as the base; arcs and any-angle later.

**Placement requirements.** Fixed items (connectors, mounting holes), both board
sides, rotation (90° steps or free), courtyard legality, assembly rules, decoupling
capacitors near their power pins, thermal grouping and functional clusters.

**Maps onto the core:** search, negotiation, Steiner construction, placement engines.

**Stays domain-specific:** geometry layer, design rule evaluation, push-and-shove,
pair and meander generation, DSN and KiCad formats. PCB is the domain most likely
to stress the `RoutingSpace` abstraction
([open question 1](architecture.md#open-questions)).

**Interchange.** Specctra DSN/SES comes first, since it is the established autorouter
interface and is how KiCad talks to Freerouting. Direct KiCad files follow, and
IPC-2581 or ODB++ on demand. Reading KiCad boards for clearance checks exists already, behind
the `kicad` feature: the copper, the nets, and the rules from the project, as KiCad's own DRC
sees them. So do reading DSN designs and writing SES sessions, behind the `dsn` feature, into
and from the same model, which KiCad's session import takes back.

`guide dsn route board.dsn -o board.ses` (also `-de board.dsn -do board.ses`, as Freerouting is
called) routes a design as KiCad exports it and writes a session for KiCad to import, which
makes the tool a drop-in for Freerouting in KiCad's DSN workflow. `pcb::Design::from_dsn` makes
the design: wiring the file marks `protect` or `fix` stays and is left out of the session, as
KiCad keeps locked tracks on import, and planes with a net become zones that their net is routed
to. What a DSN file lacks is assumed: zones take KiCad's defaults for new zones (or the board's
clearance where larger), and copper keeps the board's clearance from the edge, which
`--edge-clearance` overrides; KiCad's export doesn't carry the board's own edge clearance, so
without it `kicad-cli pcb drc` can find tracks too close to the edge. Imported back into KiCad
and refilled, the sessions of four curated boards (`TS4231-BOB`, `lisn`, `flea`, `placebo`),
routed from scratch (`flea` with `--edge-clearance 0.254`, its board's), were fully connected, and KiCad's DRC found no more errors on them than on
the same boards routed by `guide pcb route` from their `.kicad_pcb`, but for 8 solder mask
bridges on `flea`, whose openings a DSN file doesn't have. `guide pcb route` reads everything
the board has, and is the better choice where it can be used.

## FPGA

**Input.** A device description and a technology-mapped netlist, plus constraints:
fixed I/O locations, clock definitions, regions.

**Routing requirements.**

- A very large, explicit routing resource graph with capacity-one wires.
- Timing-driven costs: per-sink criticality from static timing analysis.
- Equivalent pins, for example LUT input permutation where the device allows it.
- Global networks (clocks) and pre-routed or locked nets.
- Fast, incremental rerouting.

**Placement requirements.** Discrete typed sites (LUT, flip-flop, DSP, BRAM, I/O),
packing rules within a logic block, fixed I/O, carry chains and other relative macros,
clock-region limits, and timing-driven optimization.

**Maps onto the core:** almost everything, directly. This is PathFinder's home
domain, and the graph *is* the `RoutingSpace`.

**Stays domain-specific:** device database loading, delay model, static timing
inputs, site legality and packing. Bitstream generation is out of scope; the output
is intended to hand off to tools that handle it.

**Interchange.** VTR routing resource graphs and architecture files first, because
they come with benchmarks and a reference tool to compare against. FPGA Interchange
Format and nextpnr integration next, which is the route to real devices through open
toolchains such as Project X-Ray and Project Trellis device data.

**Where it stands.** The `fpga` feature reads a routing resource graph, a packed
netlist and a placement, and writes VPR's `.route` files. `fpga::build_timing` turns
the packed netlist into a timing graph and the engine's connections, so VTR files go to
a timing-driven route with no glue: `tests/fpga_timing.rs` does exactly that for
`mult_4x4`, and checks the result with the validator. What that timing knows and
assumes:

- **Structure from the netlist.** Primitives and the interconnect between them come from
  the `.net` file's own record of how each block was filled in. Pads are known from the
  circuit's port lists, and registers and combinational paths from the architecture's
  models (or, without them, from a connected clock).
- **Delays inside blocks from the architecture file.** `fpga::read_architecture` reads
  VPR's `arch.xml` into `fpga::LogicDelays`: interconnect `delay_constant` and
  `delay_matrix` by port, primitive delays by pin, `T_setup`, `T_clock_to_Q` and
  `T_hold`, and the models that say which primitive is a register. Times become whole
  picoseconds exactly, from the decimal text. VPR's rewriting of LUT and memory classes
  into made-up blocks is reproduced, since the `.net` file names those. The caller can
  also give the numbers by hand. Anything not given is 0, or a default the caller sets,
  and is listed in the result.
- **Ideal clocks, each its own domain.** Each register is on its clock's domain and the
  pads on a virtual I/O clock, as VPR has them. `crate::timing` times each pair of
  domains against its own constraint, with VPR's per-pair criticality, and leaves paths
  between two netlist clocks untimed unless they are constrained, which is VPR's default
  without an SDC file (`DesignTiming::vpr_constraints`). Everything can still be timed as
  one clock instead. The engine takes either through the same `TimingDriven`.
- **What differs from VPR** in detail, from per-pin delays to hold timing, is listed in
  the documentation of `fpga::build_timing`'s module.

Measured on `mult_4x4` over a generated island fabric of 8 to 20 tracks (VPR's own
device graph for it isn't available), with the delays inside blocks from
`k6_N10_40nm.xml`, the architecture it was packed for: with 12 or more tracks,
timing-driven routing reaches the critical path that every connection would have alone,
2033 ps, against 2097 ps without timing, 3% faster and for less wire. With 8 and 10
tracks both come out at 2097 ps, and starting from exact delays in place of lower bounds
changes nothing. The design is small, with three logic blocks, so this says little about
timing-driven routing in general.

**Against VPR.** On the 20 MCNC circuits of VTR's regression tests, on VPR 9.0.0's own
graphs and placements, every route is legal by the validator and by VPR itself, which loads
it back and times it. The [results](benchmarks.md#fpga-routing-against-vpr) have the channel
widths, delays, wirelengths and runtimes, and what it took to get there. The same holds on
[VTR's Verilog suite](benchmarks.md#vtrs-verilog-benchmarks), the 21 circuits of
`vtr_reg_qor_chain` on VTR's flagship architecture, with carry chains, memories and
multipliers over several tiles. There `guide` needs 4.7% fewer tracks than VPR, gives
critical paths 0.2% shorter and the same wire, and takes 0.38 times as long, and on MCNC 6.0%
fewer, 1.6% shorter, 5.3% less and 0.30 times, since `RrGraph::engine_options` was [tuned on
both](benchmarks.md#tuning-the-negotiation-on-both-suites). Constant nets are left
unrouted as VPR leaves them (`fpga::constant_nets`), and the primitives VPR makes up for a LUT
or memory class keep their own delays (`fpga::LogicDelays::primitive_name`). Titan23 has not
been run.

## ASIC

Very close to both of the above. Global routing is negotiated congestion over a grid
of capacitated cells, which `guide` should handle with little domain code. Detailed
routing adds track assignment, complex design rules at advanced nodes, and pin access.
Global routing and placement are realistic targets, given the ISPD benchmarks. A
signoff-grade advanced-node detailed router is not a goal for now. Interchange would
be LEF/DEF.

**Where it stands.** The `asic` feature does global routing on the ISPD 2007 and 2008 contests'
problems: it reads their designs (`asic::read_design`), routes them (`asic::route_design`),
writes routes in their format, and scores routes from any router as the 2008 contest's
`eval2008.pl` does (`asic::evaluate`), which agrees with the script on every file both have
scored. The flow is the one most of the contests' routers use:

- **The plane.** Each layer's capacity becomes tracks, and the layers are added up onto one
  plane, an `asic::PlanarSpace` whose nodes are cells, twice over, for arriving along a row or a
  column, so that a turn, which takes a via, can cost more than a straight step.
- **Pattern routes first.** Each net starts from L and Z shapes along a rectilinear Steiner tree
  of its pins, sharing what they can, against the use of the nets before it: `seed_routes`, so
  that the engine only routes again what overflows.
- **The engine negotiates the rest**, longest nets first, by maze routing within a box around
  each net's pins that grows while the net stays on an edge that overflows, with
  `EngineOptions::crowding` pricing edges as they fill, to the 8th power of how full they would
  be: mildly in a first try, which settles most designs in a few cheap iterations, and steeply
  from the start again on a design that the first try does not settle quickly. When
  negotiation stalls, with a few edges one net over, it starts again from its best routes with
  the history it has learned and the present factor reset; that is what makes the hardest
  designs converge. On a design that cannot be routed without overflow, the nets on what is
  left are routed again priced at the overflow each would add
  (`EngineOptions::marginal_overuse`). A last pass routes again, with `Engine::rip_up`, the nets
  that took detours, by their shortest ways that overflow nothing.
- **Layers** are then given to each net's tree exactly for the fewest vias that fit, by dynamic
  programming over the tree as in COLA, nets with the least wire per pin first, each on the
  emptier of two layers that take as many vias, and the nets on an edge that overflows on a
  layer while another has room are given layers again.

Nothing of the engine or search code is specific to it: the domain is the space, the pattern
routes, the layer assignment and the file formats. The engine gained general things for it,
which change nothing for a run that does not use them: `EngineOptions::crowding`, off by
default, `EngineOptions::crowding_exponent`, the square by default,
`EngineOptions::marginal_overuse`, off by default, `Engine::rip_up`, and
`RoutingSpace::lower_bound_to_targets`, whose default is what searches did before. The
[results](benchmarks.md#asic-global-routing-against-the-ispd-contests) compare it with the
contests' winners and later routers: on the ISPD 2008 designs, all 12 that they route free of
overflow are free of it here, with 0.35% more wirelength than the contest's winner and 1.6%
more than NCTU-GR 2.0, in 0.4 to 4.3 times the CPU time of NCTU-GR 2.0's single-threaded router
on the ten that converge without restarts. On the four that no router solves, it is within
1.2% of the contest's winner on newblue3, within the contest's range on bigblue4 and newblue4,
and behind its best on newblue7.

## Multi-agent path finding

Warehouse robots, trains and game units route over a shared graph, with space-time
nodes as the resource: a cell may hold only one agent per time step. A `RoutingSpace`
over (location, time) expresses that directly, and prioritized planning and
conflict-based search are close relatives of rip-up and negotiation. Domain code is
small, so this is the cheapest strong proof that the core is general (roadmap M10).

**Implemented** in `guide::mapf`, behind the `mapf` feature, as the classic problem of Stern
et al. (SoCS 2019): agents on a graph or a 4-connected grid, one move or wait per time step,
no vertex or swap conflicts, agents staying on their goals, and the sum of costs as the
objective, with the makespan reported.

- **Two ways to plan, one negotiation.** `TimeSpace` is a `RoutingSpace` over (vertex, time)
  states, keyed through `InternedSlots`, whose resources are a vertex at a time and an
  undirected edge during a move, each of capacity 1, so the engine's overuse is exactly the set
  of conflicts; each agent is a net of two terminals, its start at time 0 and its goal, and
  after the goal a route goes on through a stay at each time to a horizon. The engine
  negotiates over it unchanged (`Method::TimeExpanded`). The default, `Method::SafeIntervals`,
  plans each agent over safe intervals instead (SIPP: a state is a vertex and an interval of
  time over which nobody arrives or leaves), with occupied intervals priced at the engine's
  price rather than forbidden, as SIPPS does, and runs the engine's negotiation loop, its
  schedule, history, limits and cancel flag, over a table kept per vertex in time order.
- **Why not through the engine.** A safe-interval search has to ask when a vertex is next free,
  or next changes price; the engine's resource table is a hash map from a resource to who holds
  it, which can only answer one time step at a time. Giving the engine an ordered index per
  vertex for one domain would cost every other domain, so the loop, about 100 lines, is
  repeated in `mapf` instead, with the same formula and schedule; the engine is unchanged.
  On the large MovingAI maps, where paths are a thousand steps long, the time-expanded space
  took seconds to minutes an iteration and solved none of maze-128-128-2; safe intervals made
  every map faster and every sum of costs lower.
- **A first solution.** LaCAM over PIBT, one step at a time for every agent, finds a solution in
  milliseconds before negotiation starts, to fall back on when negotiation hits its iteration
  limit or is stopped. Negotiation's solution is the one returned when it settles.
- **Shortening.** After a legal solution, each agent is planned again against the others as hard
  reservations, over safe intervals, and keeps what arrives sooner.
- **Domain code** is about 2600 lines without tests or comments, error types included: the
  graph, both spaces, the safe-interval planner and its negotiation, LaCAM, the solve loop,
  shortening, the checker, and a reader for the MovingAI `.map` and `.scen` files.
- **Checking** is `mapf::check`, which takes plans as vertices per time step and knows nothing
  of the space or the engine.

On six MovingAI maps, 25 scenarios each at ten agent counts, `guide` solved all 1500
instances within a minute on one thread, as did LaCAM3, against 1237 for EECBS and 308 for the
optimal CBSH2-RTC, with the lowest sums of costs of the suboptimal solvers nearly everywhere,
within 1.4% of the optimum wherever CBSH2-RTC found one, and a first solution in milliseconds.
On all 33 maps, one random and one even scenario each at four agent counts, it solved 231 of
240 under 30 seconds, as did LaCAM3, with lower sums of costs on every map but those where
negotiation ran out of time and it returned its first solution unrefined, and below the MAPF
tracker's best known solution on 132. [The results](benchmarks.md#multi-agent-path-finding-on-the-movingai-benchmarks)
have the tables and what this does not show.

## Other candidates

These are listed to keep the abstractions honest, with no commitment to build them.

- **Qubit mapping:** placing logical qubits on a device and inserting SWAPs. It has
  the same structure of initial placement plus time-expanded routing.
- **Wire harnesses, cable trays, piping and HVAC:** 3D routing around obstacles with
  bundling benefits, where sharing a route lowers cost rather than raising it.
- **Microfluidics:** flow-channel routing with the same layout character as a PCB.
- **Network flow and traffic:** related, but multi-commodity flow solvers often beat
  path-based methods there, so `guide` may not be the right tool.
