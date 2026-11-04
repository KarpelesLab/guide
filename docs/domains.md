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
