# Roadmap

This is a plan, not a commitment: ordering and scope will change as we learn.
Each milestone has exit criteria, so "done" is checkable. Milestones are not tied
to version numbers; release-plz assigns those from the commit history.

Design background is in [docs/architecture.md](docs/architecture.md).

## Sequencing rationale

**The engine first, then FPGA routing, then PCB.** This is deliberately not
"PCB first", even though PCB may be the larger audience.

FPGA routing is a pure graph problem: no geometry layer is needed, and public
benchmarks and mature open-source tools (VPR, nextpnr) provide objective
comparisons. That makes it the cheapest honest test of the shared engine. PCB
routing then adds geometry on top of an engine already known to work, and tests
whether the abstraction survives a very different domain. If it does not, we want
to find out before the API has settled.

Routing comes before placement because placement quality can only be judged by
how well the result routes.

## Measuring "best"

Every domain milestone adds a row to a benchmark report in the repository, and
regressions fail review. The metrics:

| Domain | Quality metrics | Compared against | Benchmarks |
| --- | --- | --- | --- |
| FPGA routing | Routability, minimum channel width, critical-path delay, wirelength, runtime | VPR, nextpnr | VTR benchmarks, Titan23 |
| FPGA placement | Post-route wirelength and delay, legality, runtime | VPR, nextpnr | VTR benchmarks, ISPD 2016/2017 contest sets |
| PCB routing | Completion, DRC violations, via count, track length, runtime | Freerouting | Open-source KiCad boards (no standard suite exists, so we curate one: [testdata/kicad](testdata/kicad/README.md)) |
| PCB placement | Routed completion and length on the result, courtyard legality | Hand-placed originals | Same curated boards |
| ASIC global routing | Overflow, wirelength | Published contest results | ISPD 2007/2008 |

Results come from a validator that does not share search code with the router.

reticle and cadlab already have their own routers and are expected to adopt `guide`
([target consumers](docs/architecture.md#target-consumers)). So there is a further
bar: `guide` has to match or beat each one on its own designs and boards, or
adopting it would be a regression.

## Milestones

### M0: Scaffold (done)

Crate at v0.0.0, CI, release-plz, documentation, this roadmap.

### M1: Core model and search (done)

The vocabulary everything else uses, and single-net routing.

- Nets, terminals, routes, and the [numeric types](docs/architecture.md#decisions)
  shared by all domains.
- The `RoutingSpace` abstraction ([architecture](docs/architecture.md#core-abstractions)).
- Dijkstra and A\* with reusable search state (no per-search allocation).
- A simple grid space and a generic explicit-graph space.
- Seeded, platform-independent randomness.

**Exit:** optimal single-net routes on both spaces, verified against brute force
by property tests; benchmarks exist for search throughput.

### M2: Multi-net engine (done)

The heart of the project.

- Multi-terminal nets: incremental tree growth, then Steiner tree construction.
- Negotiated congestion (PathFinder), rip-up and reroute.
- Validator: connectivity and capacity checking, independent of the router.
- Partial results with failure reports; budgets and cancellation.
- Deterministic parallel routing.

**Exit:** dense grid and graph problems route legally, or report unroutable nets
correctly; identical output across thread counts and OSes, enforced in CI.

### M3: FPGA routing (done)

First real domain and the first test of the abstraction.

- Import VTR routing resource graphs and pre-placed designs, and write routes that VPR
  accepts. Done, through `fpga::resolve_nets` and `fpga::write_route`.
- Delays inside blocks, and which primitives are registers, from VTR architecture files.
  Done, through `fpga::read_architecture`.
- Non-configurable edges (shorts), taken as a whole set by the engine and checked by the
  validator. Done, through `RoutingSpace::non_configurable`.
- Static timing analysis with a pluggable delay model.
- Multiple clock domains, each against its own constraint, with paths between clocks
  untimed unless constrained, as VPR does without an SDC file. Done, through
  `timing::Constraints::per_clock` and `fpga::DesignTiming::vpr_constraints`.
- Timing-driven routing using net criticality, optionally going on after the first legal
  result to shorten the critical path (`EngineOptions::timing_iterations`, measured in
  `docs/benchmarks.md`).
- Fixed pins and pre-routed nets (clocks, for example). Done in the engine:
  `Engine::fix_routes` takes whole or partial routes, and `validate::check_fixed` checks
  that a result kept them.

**Exit:** the VTR benchmarks route legally, and first results are published
against VPR for channel width, delay and runtime.

Met on the 20 MCNC circuits that VTR's regression tests route on `k6_N10_40nm`, the set VPR's
papers use, against VPR 9.0.0 on its own placements and graphs
([results](docs/benchmarks.md#fpga-routing-against-vpr)). Every route is legal by the validator
and by VPR, which loads and times each one. Against VPR, `guide` needs 4.4% fewer tracks, gives
critical paths 1.3% shorter and 5.5% less wire, and takes 0.28 times as long, since a map
lookahead and incremental rerouting; it first took 3.2 times as long. VTR's Verilog suite,
with memories and multipliers, and Titan23 are still to be run.

### M4: Geometry layer (done)

Everything PCB routing needs that FPGA routing did not, as an optional feature.
polyclip already provides polygons with arcs, offsetting, booleans and exact distance
queries, so this milestone builds on it instead of reimplementing them.

- `i64` coordinates and conversions to and from polyclip, kept out of the public API.
- Spatial index and clearance queries.
- Design rule model: widths, clearances, via definitions, per-net-class and
  per-layer overrides.

- Reading KiCad boards and their rules, to check them (`kicad` feature).

**Exit:** a clearance checker that agrees with KiCad's DRC on the curated boards
(differences investigated, each one explained or fixed).

Met on nine [curated boards](testdata/kicad/README.md), under their own rules and with raised
clearances: of KiCad 10.0.3's 3450 findings we make 3448, and all 23 differences, 2 of KiCad's
and 21 of ours, are explained, mostly by limits of KiCad's own check
([results](docs/benchmarks.md#pcb-clearance-checks-against-kicad)).

### M5: PCB routing MVP

- Specctra DSN import and SES export, giving KiCad interoperability through its
  existing Freerouting workflow. Done, behind the `dsn` feature: `dsn::read` gives the same
  board and rules as `kicad::read`, and on the nine curated boards, as KiCad exports them, finds
  what the `.kicad_pcb` does but for differences that are each explained by what DSN doesn't
  carry, such as zone fills and holes, or by KiCad's rounding. Sessions from `write_session`
  import into KiCad with every straight track and via exact, and KiCad's DRC unchanged
  ([results](docs/benchmarks.md#pcb-dsn-designs-against-kicad-boards)).
- Multi-layer, grid-free routing over the geometry layer: through vias, keepouts,
  fixed pre-routes, net classes. Done, in `pcb::route`, with octilinear tracks, through vias
  and vias in pads, the board's edge and solder mask openings.
- Global routing followed by detailed routing. Done: the engine routes tiles globally, and
  each net is then routed in detail inside its corridor, with rip-up
  ([design](docs/architecture.md#decisions)).
- Post-routing cleanup: via removal, track shortening. Done: nets are rerouted with dearer
  vias, and runs of tracks pulled tight.
- Writing routes back into a KiCad board, to check them with KiCad's own DRC. Done, in
  `kicad::replace_routing`.

**Exit:** the curated boards are completed with zero DRC violations according to
KiCad itself, with first results published against Freerouting.

Eight of the nine curated boards, stripped of their tracks, vias and zone fills, are routed
completely, ground and power included, and KiCad 10.0.3 finds nothing wrong with any of the
nine: no violation, and no missing connection but on the ninth. That one, `nk3-nrf-mini`, has 29
of its 30 nets, with escape routing from its 0.5 mm-pitch aQFN. The last, `/LED_R`, can't be
routed without a solder mask bridge that KiCad reports: its pad lies inside a mask opening drawn
across another net's pad, where the original board's own via is reported too
([results](docs/benchmarks.md#pcb-routing-of-the-curated-boards)).

The first results against Freerouting 2.5.0 are published, from the same stripped boards and
checked by the same KiCad DRC ([results](docs/benchmarks.md#pcb-routing-against-freerouting)).
Freerouting completes seven of the nine, leaves one connection on `flea`, and makes 44 of
`nk3-nrf-mini`'s 83 connections, to the 56 `guide` made then and the 82 it makes now. KiCad finds violations in its results on six
boards, nearly all of rules that the DSN it is given doesn't carry. On the seven boards both
complete, `guide`'s track is 6% shorter, and its vias about as many, 168 to 172, though more on
three boards. On one thread it was faster on seven boards, and slower on `flea` and
`nk3-nrf-mini`, before speedups that cut those two to a quarter and a third. Left for the exit:
`nk3-nrf-mini`'s `/LED_R`, which the board itself makes impossible without a violation, so M5 is
done but for a fault in that board's design.

### M6: PCB routing, advanced

Each item is independently useful, and they can land in any order.

- Push-and-shove detailed routing.
- Differential pairs.
- Length and skew matching.
- BGA and fine-pitch escape routing. Started: pads that nothing passes between form
  clusters, and those that can't get out on their own layers get a through via, placed on a
  fine grid to leave room for the others, or where no via fits, their only way out, before
  anything else is routed. Through vias only.
- Blind, buried and micro vias; impedance-driven width per layer stack.
- Copper zones and plane-aware routing.
- Net topology constraints (daisy chain, star, T).
- Direct KiCad board-file reading for routing, and writing routes back. Reading for
  clearance checks came with M4.

**Exit:** a real high-density board (DDR memory or equivalent) routes with its
constraints satisfied; results beat Freerouting on the curated suite.

### M7: Generic placement

- Placement problem model: movable and fixed items, sites or free space, rotations,
  macros, region constraints.
- Simulated annealing with an adaptive schedule.
- Analytic global placement, then legalization, then detailed placement.
- Congestion estimates from the routing engine feed back into placement.

**Exit:** on synthetic problems, legal placements with wirelength close to known
reference solutions; deterministic like the router.

### M8: Domain placement

- **FPGA:** packing-aware site legalization, fixed I/O, carry chains and other
  macros, clock regions, timing-driven optimization.
- **PCB:** fixed connectors and mounting holes, courtyards, both board sides, 90°
  or free rotation, decoupling capacitors kept near their pins, thermal grouping.

**Exit:** FPGA placement competitive with VPR after routing; PCB placement that
routes to completion on the curated boards with the original placement discarded.

### M9: Co-optimization

Placement and routing as a single loop rather than two sequential passes.

- Local re-placement around failed routes.
- Incremental re-route after engineering changes, preserving existing work.
- Pin and gate swapping where the netlist permits (FPGA pin permutation, MCU pin
  muxing, memory byte lanes).

**Exit:** measurable completion or quality gains over the one-pass flow on the
hardest benchmarks; small netlist changes cause small layout changes.

### M10: More domains and 1.0

- At least one domain outside electronics (multi-agent path finding is the leading
  candidate), as proof that the core is general.
- ASIC global routing against the ISPD benchmarks.
- `nextpnr`/FPGA Interchange and IPC-2581 interop as demand dictates.
- Command-line tool; API review and stabilization.

**Exit (1.0):** published benchmark results in at least three domains; a documented,
stable public API; a semver policy that covers result determinism.

## Not yet scheduled

Ideas worth keeping, without a place in the plan yet: learned heuristics for net
ordering or congestion prediction; GPU acceleration for analytic placement;
any-angle and topological (rubber-band) PCB routing; 3D and flex/rigid-flex boards;
WebAssembly builds; bindings for other languages.
