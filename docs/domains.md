# Domains

What each target domain needs, how it maps onto the core, and what stays
domain-specific. **None of this is implemented yet**; see the
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
| Roadmap priority | Primary | Primary | Later | Later | Exploratory |

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
IPC-2581 or ODB++ on demand.

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
  circuit's port lists, registers from a connected clock, and anything else is
  combinational from every input to every output.
- **Delays inside blocks from the caller.** Architecture files are not read yet, so
  `fpga::LogicDelays` takes the numbers they hold (interconnect `delay_constant`, LUT
  delays, `T_setup`, `T_clock_to_Q`) by the names the netlist uses. Anything not given
  is 0, or a default the caller sets, and is listed in the result. With nothing given,
  only routing delay counts, which still says which connections lie on which
  register-to-register paths.
- **One ideal clock.** Every clock is timed as one, so paths between unrelated clocks are
  timed (pessimistically) where VPR's default would not time them. One clock's registers
  can be timed alone instead. Real multi-clock analysis needs clock domains in
  `crate::timing` and the engine.
- **What differs from VPR** in detail, from per-pin delays to hold timing, is listed in
  the documentation of `fpga::build_timing`'s module.

Measured on `mult_4x4` over a generated island fabric of 8 to 20 tracks (VPR's own
device graph for it isn't available): with 12 or more tracks, routes with and without
timing both reach the critical path that every connection would have alone, 1840 ps under
the test's made-up delays. With 8 and 10 tracks, timing-driven routing came out 3% and
1.4% slower than routing without timing (2165 against 2100 ps, 1865 against 1840 ps), and
starting it from exact delays in place of lower bounds changed nothing. The design is
small, with three logic blocks, so this says little either way about timing-driven
routing. It does say that, on congested designs, the engine stopping at the first legal
iteration leaves timing to chance, which is the next thing to look at. Comparing with
VPR needs VPR's device graphs and architecture delays.

## ASIC

Very close to both of the above. Global routing is negotiated congestion over a grid
of capacitated cells, which `guide` should handle with little domain code. Detailed
routing adds track assignment, complex design rules at advanced nodes, and pin access.
Global routing and placement are realistic targets, given the ISPD benchmarks. A
signoff-grade advanced-node detailed router is not a goal for now. Interchange would
be LEF/DEF.

## Multi-agent path finding

Warehouse robots, trains and game units route over a shared graph, with space-time
nodes as the resource: a cell may hold only one agent per time step. A `RoutingSpace`
over (location, time) expresses that directly, and prioritized planning and
conflict-based search are close relatives of rip-up and negotiation. Domain code is
small, so this is the cheapest strong proof that the core is general (roadmap M10).

## Other candidates

These are listed to keep the abstractions honest, with no commitment to build them.

- **Qubit mapping:** placing logical qubits on a device and inserting SWAPs. It has
  the same structure of initial placement plus time-expanded routing.
- **Wire harnesses, cable trays, piping and HVAC:** 3D routing around obstacles with
  bundling benefits, where sharing a route lowers cost rather than raising it.
- **Microfluidics:** flow-channel routing with the same layout character as a PCB.
- **Network flow and traffic:** related, but multi-commodity flow solvers often beat
  path-based methods there, so `guide` may not be the right tool.
