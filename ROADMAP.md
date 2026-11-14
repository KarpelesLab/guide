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
| Multi-agent path finding | Success rate within a time limit, sum of costs against the optimum and a lower bound, runtime | CBSH2-RTC, EECBS, LaCAM3 | MovingAI MAPF benchmarks |

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
and by VPR, which loads and times each one. Against VPR, `guide` needs 6.0% fewer tracks, gives
critical paths 1.6% shorter and 5.3% less wire, and takes 0.30 times as long, since a map
lookahead and incremental rerouting; it first took 3.2 times as long. Before the
negotiation was tuned on both suites, below, it needed 4.1% fewer tracks, with paths 1.3%
shorter, 5.5% less wire and 0.28 times VPR's time.

Since then also on VTR's Verilog suite, the 21 circuits of `vtr_reg_qor_chain` on
`k6_frac_N10_frac_chain_mem32K_40nm`, with fracturable LUTs, carry chains, memories and
multipliers over several tiles, up to 7600 clusters on a 103 by 103 grid
([results](docs/benchmarks.md#vtrs-verilog-benchmarks)). That took reading blocks over
several tiles, leaving constant nets unrouted as VPR does, and telling apart the primitives
VPR makes up for each LUT and memory class. Every route is legal by VPR. Here `guide` first
needed 1.4% more tracks than VPR, with critical paths 0.3% longer. With the negotiation
tuned on half of each suite (a history of two tiles of wire, a present factor growth of
1.5, and giving up early on widths that will not route, as VPR's failure predictor does),
it needs 4.7% fewer, gives critical paths 0.2% shorter and the same wire, and takes 0.38
times as long, against 0.35. Titan23 is still to be run.

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

### M5: PCB routing MVP (done)

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
done but for a fault in that board's design. To revisit: a route for `/LED_R` that KiCad accepts,
if one can be found, or a better way to treat a board whose own design breaks its rules.

### M6: PCB routing, advanced (done)

Each item is independently useful, and they can land in any order.

- Push-and-shove detailed routing. Not done, and not needed for the exit: what the DRAM boards
  needed was pairs routed first that move out of the way of the last nets, in rip-up, and
  are routed again side by side, below.
- Differential pairs. Done, in `pcb::route`: pairs found by their names as KiCad does, at
  their class's width and gap, are routed side by side after everything else, in place of
  their two nets' routes, kept only where they connect what those did, so that no other net
  ever changes: a search for the centreline wide enough for both tracks, offset into two
  mitred tracks, vias side by side, and each net's own fan-out near its pads. Both curated
  USB pairs that the router can reach run side by side for about half their length, against
  28% and 73% by hand, and KiCad's DRC accepts them
  ([results](docs/benchmarks.md#pcb-differential-pairs-and-lengths)). Pairs can be routed
  first instead, `RouteOptions::pairs_first`, which the DRAM boards need, and a pair whose end is in
  a BGA fans out of it apart. A pair whose ends are on different layers changes layers side by
  side: `ddr5-testbed`'s `TDQS`, balls on top and fingers below, runs side by side for 63% of
  its length, against 15% by hand. Routed first, pairs keep a place for a via free beside the
  pads of nets with zones next to theirs, such as a ground finger beside a pair's, and a net
  left for the passes on the finer lattice may cross them in rip-up: the pair is taken out, and
  routed again side by side around the net, moving other nets in turn. Not yet: a
  pair whose pads leave no room for both tracks to get out together, as `nk3-nrf-mini`'s, which
  stays as two nets.
- Length and skew matching. Done: lengths, skews within pairs and matched groups, from
  `.kicad_dru` custom rules or the API, met with trombone meanders added after routing,
  checked exactly, together on both tracks of a pair. `isi-channel`'s three channels come out
  at their hand-routed 55, 150 and 300 mm, both nets of each within 50 nm of the target.
  Widths and pairs' gaps come from custom rules too. A net left short where its neighbours'
  meanders took the room gets it back: theirs are taken out, it is meandered first, and they
  after it. On the DRAM boards, all four matched buses are met to within 50 nm.
- BGA and fine-pitch escape routing. Started: pads that nothing passes between form
  clusters, and those that can't get out on their own layers get a through via, placed on a
  fine grid to leave room for the others, or where no via fits, their only way out, before
  anything else is routed. Through vias only.
- Blind, buried and micro vias; impedance-driven width per layer stack. Neither DRAM board
  needs them.
- Copper zones and plane-aware routing. Done: `pcb::fill_zones` fills zones as KiCad does,
  with clearances, thermal reliefs, minimum widths, priorities and island removal, within 1% of
  KiCad 10's own fills on the curated boards, and KiCad's DRC finds nothing wrong with them.
  `pcb::route` keeps zones' outlines, and connects their nets to them: a net's pads that its
  fill reaches are one terminal with places in the fill near its other pads, so that it is
  routed as stubs, track and via, to the fill; inner-layer fills are planes that other nets
  pay to cross; and after routing, zones are filled again, and a net left in pieces is joined
  up again by stubs or tracks. All nine curated boards route as completely as before, the
  ground and power nets through their zones, with KiCad's DRC clean, of guide's fills and of
  KiCad's own refill; `GB-LIVE32` in 24 s rather than 34, with 2283 mm of track, against 2705
  and 2315 by hand, and 165 vias against 142 and 140
  ([results](docs/benchmarks.md#pcb-zones-and-planes)). A zone sets how many spokes its
  reliefs have and where the first points. Not yet: hatched fills, fillets, and planes split
  between nets on one layer, which are filled as any zones.
- What a host's board says beyond pads and nets, for a program that drives `pcb::route` with
  a board model of its own. Done, each off or empty by default, which routes as before: no
  via in a pad with no hole (`RouteOptions::vias_in_pads`); zones filled by the host
  (`Zones::fills`), which routing plans on in place of its own fill, and `Shape::rings` and
  `Shape::outline` to read any shape back; zones that are planes or fixed copper, by zone
  (`CopperZone::plane`) or by how much of its net a zone covers
  (`RouteOptions::plane_coverage`); keepouts that except nets (`drc::Keepout::except`); the
  layers a run's and a net's tracks may be on (`RouteOptions::layers`, `NetSpec::layers`); pad
  stacks (`drc::Pad::stack`); net ties (`drc::Board::ties`); and a clearance and a track width
  inside an area (`RuleSpec::areas`), of which routing applies the clearance. Not measured on
  real boards yet. A net too wide to leave one of its pads is routed narrower, at one width
  for the whole net (`RouteOptions::neck_down`). Not yet: tracks routed at an area's width,
  or narrower only near pads, and several via sizes for a net, which the search, with one
  width and one via for each net, has no place for; nor a fanout planned for a whole
  footprint, which is what the pads that are still shut in with vias kept out of pads need
  ([benchmarks](docs/benchmarks.md#pcb-routing-after-the-release-candidate)).
- Net topology constraints (daisy chain, star, T). Not needed by the DRAM boards, whose
  signals are all point to point.
- Direct KiCad board-file reading for routing, and writing routes back. Reading for
  clearance checks came with M4.

**Exit:** a real high-density board (DDR memory or equivalent) routes with its
constraints satisfied; results beat Freerouting on the curated suite.

Met. Two DRAM boards are curated, Antmicro's `lpddr4-testbed` and `ddr5-testbed`, a BGA DRAM
broken out to a SO-DIMM edge on six layers with 0.1 mm tracks, and both route completely,
ground and power through their planes, with KiCad's DRC clean, in 90 and 137 s, with fewer vias
and less track than by hand ([results](docs/benchmarks.md#pcb-routing-of-dram-boards)). With
their buses' lengths and pairs as their hand routing has them, as custom rules, both meet them
all. `lpddr4-testbed` routes completely, both its buses, 22 and 10 nets, matched within 50 nm.
`ddr5-testbed` routes completely too, all 255 connections, its pairs routed first and its nets
narrower, with its three pairs side by side for 53%, 63% and 58% of their length, against 70%,
15% and 76% by hand, and both its buses, 12 and 17 nets, matched within 50 nm, where the hand
routing misses them by 0.56 and 2.7 mm; KiCad's DRC finds no violation and no connection
missing. Its last three connections, `RESET_N`, `CA3` and a ground finger, came from pairs
routed first that move out of the way of the last nets in rip-up and are routed again side by
side, and a larger budget for those nets' searches; other variations, measured, left one to
four.

Against Freerouting, run again on all eleven boards with current guide
([results](docs/benchmarks.md#pcb-routing-against-freerouting)): on completion with no
violation, guide ties on the four boards that a run of Freerouting completes with none, and
beats it on the other seven, the DRAM boards included, stripped of their zones: 344 of 344
connections and 255 of 255, against Freerouting's best 337 and 245, which break rules or leave
more, and 82 of 83 on `nk3-nrf-mini` against 42. guide's track is shorter on every board but
`nk3-nrf-mini`, where it makes 40 more connections, and it is faster on all but that one,
where the two take as long, but it has more vias on seven.

Not done of M6's list: push-and-shove, which the exit didn't need, blind, buried and micro
vias, impedance, topologies, and pairs whose pads leave no room to get out together.

### M7: Generic placement (done)

- Placement problem model: movable and fixed items, sites or free space, rotations,
  macros, region constraints. Done, in `place::PlacementProblem`: typed sites or a free-space
  area with a pitch, quarter turns, rigid macros, rectangular regions, weighted nets, and a cost
  that is a sum over nets of a `CostTerm`, half-perimeter wirelength by default. `place::check`
  says whether a placement is legal, sharing no code with the placers.
- Simulated annealing with an adaptive schedule. Done, in `place::Annealer`: VPR's schedule,
  range limit and incremental cost, a temperature at a time, with a move budget and a cancel
  flag; VPR's median moves, and macros that push the macros they land on.
- Analytic global placement, then legalization, then detailed placement. Done, in
  `place::place_analytic`: bound-to-bound quadratic placement by conjugate gradients, spread by
  recursive bisection as in SimPL, Tetris-like legalization with rows after Abacus in free
  space, and low-temperature annealing.
- Congestion estimates from the routing engine feed back into placement. The interface is done:
  `place::CongestionEstimate`, with a RUDY map, `place::CongestionGrid`, that a router can fill
  instead, priced by `place::CongestionCost`. Filling it from the global router, and iterating,
  is M9's.

**Exit:** on synthetic problems, legal placements with wirelength close to known
reference solutions; deterministic like the router.

Met on problems built with a known optimum, grids, chains and Peko-style netlists on sites and in
free space, up to 1152 items ([results](docs/benchmarks.md#placement-on-synthetic-problems)),
taking "close" to mean within 25% of the optimum on average on every family and within 10% over
all of them. Every placement is legal, and the same seed gives the same placement, on any number
of threads. Analytic placement, the default, finds the optimum on every grid, with pads, macros
or neither, is 4% above it on a chain, 18% to 22% above on Peko problems, as good as annealing
with ten times its default moves, and 2% to 19% above in free space, including 144 squares
packed exactly full, which went from 1.70 to 1.19 once rows after Abacus replaced the packing
that ignored the global placement; 8.8% as a geometric mean, from 16%. Annealing alone at its
default effort is 25% to 80% above, and 0% to 33% with ten times the moves. Left for later: an
unanchored grid packed exactly full that comes out sheared (21% above on two seeds of three). The
FPGA hookup, building a problem from a VTR netlist and architecture, came with M8.

### M8: Domain placement (done)

- **FPGA:** packing-aware site legalization, fixed I/O, carry chains and other
  macros, clock regions, timing-driven optimization. Done but for clock regions, in
  `fpga::build_place_problem` and `fpga::place_design`: a placement problem from VPR's packed
  netlist on the device of its routing graph, with a site per position of each tile, pins on the
  tiles VPR puts them on, carry chains from the architecture's direct connections as macros,
  and blocks fixed by a `.place` file as with `--fix_clusters`; VPR's bounding-box and
  timing cost, with criticalities from a timing analysis each temperature and delays from a
  delta-delay table searched on the graph; analytic global placement, then annealing that
  starts cold; a congestion term, the expected use of each channel against the graph's tracks
  each temperature, and `place::Inflation` for analytic placement; and `fpga::write_placement`
  for VPR. VTR's architectures have no clock regions, so nothing models them yet.
- **PCB:** fixed connectors and mounting holes, courtyards, both board sides, 90°
  or free rotation, decoupling capacitors kept near their pins, thermal grouping. Done but for
  free rotation, in `pcb::place::place_components`, with `kicad::placement_design` and
  `kicad::replace_placement`: footprints from a KiCad board with their courtyards, or their
  pads' and silkscreen's box, and their pins at their real offsets; fixed if locked, not
  assembled, mounting holes, connectors, fiducials, switches, or on the board's edge; the edge,
  fixed parts and keepouts rasterized into a free-space `place` problem, and the two sides
  placed in passes, parts through the board taking both, surface-mount parts moving to the
  other side when theirs is too full; half-perimeter wirelength on pins, pins of zones' nets
  drawn into their zones, decoupling capacitors tied to their supply's power pins, optional
  thermal groups, and a RUDY routability term, by analytic placement then annealing; written
  back into the board's text, flipped footprints as KiCad flips them. Parts turn by quarter
  turns only.

**Exit:** FPGA placement competitive with VPR after routing; PCB placement that
routes to completion on the curated boards with the original placement discarded.

The FPGA half is met ([results](docs/benchmarks.md#fpga-placement-against-vpr)): on VTR's 20
MCNC circuits and 21 Verilog benchmarks, from VPR's packing, VPR 9.0.0 accepts every placement
of `guide`'s, and routed by VPR's router they need channels 1.9% and 2.3% narrower than VPR's
own placements, with critical paths 0.7% and 2.0% shorter and 5.6% and 2.7% less wire, in 1.5
and 0.6 times VPR's placement time; `guide`'s router finds the same. Pricing congestion took
channel width from 0.987 and 1.001 of VPR's to 0.981 and 0.977, 1.5% on every one of five
seeds, with as much wire and delay. The large Verilog circuits are now within the spread of
VPR's own seeds, but blob_merge and mkSMAdapter4B still need 7% and 16% wider channels than
VPR's placement, where timing costs wire.

The PCB half is met ([results](docs/benchmarks.md#pcb-placement-of-the-curated-boards)): on all
eleven curated boards, with the placement of every movable footprint discarded, `guide` places
them again in under half a second, and `pcb::route` routes every board to completion, with its
zones; KiCad 10.0.3 finds no violation of anything that moved or was routed, courtyards and
silkscreen included, and no connection missing. That includes `nk3-nrf-mini`, which as placed
by hand leaves a net unrouted. The hand placements route with fewer vias on seven boards of the
nine that have anything to move, and shorter track on six.

### M9: Co-optimization

Placement and routing as a single loop rather than two sequential passes.

- Local re-placement around failed routes. Done on PCBs, in `kicad::place_and_route`: after the
  first placement and routing, rounds that place again, by `pcb::place::refine_components`, a
  window of the parts on the nets that cost most in vias and detours or failed, with their nets
  weighed more, congestion from a map of what routing used, and parts sent to the side where
  most of their connections are; reroute only what moved; and keep the round if it routes
  better, by connections, starved thermal reliefs, vias and track. The first placement's sides
  now go first to parts that nothing holds to theirs. Done on FPGAs in `fpga::place_route_loop`:
  where a routing fails, `fpga::RouterCongestion` takes the router's history of overuse,
  iteration by iteration, and the wire it used, into the placer's estimate of congestion, and
  `fpga::refine_placement` anneals again at a low temperature, timing-driven, the blocks within a
  window around what was overused; then the design is routed again.
- Incremental re-route after engineering changes, preserving existing work. On PCBs,
  `pcb::eco::reroute` keeps a routed board's copper that a change leaves alone as fixed copper
  and routes only what is left apart, and `pcb::eco::relieve` gives starved thermal reliefs a
  second spoke. Elsewhere, done in
  `Engine::seed_routes`, which starts each net from the route it had, cut back to what is still
  a step of the space and still leads to a terminal, and grown from where terminals were added;
  `Engine::seed_history` for the negotiation's history; `eco::reroute` and
  `eco::stable_options`, which route what changed around what did not; and, for placement,
  `place::place_incremental`, which puts items back where they were when that is still legal,
  places new ones near what they connect to, and improves with a `Displacement` cost term that
  charges for moving. `fpga::route_seeds` and `fpga::placement_priors` carry routes and a
  placement over to a changed netlist by name. `eco::route_change` and `place::placement_change`
  measure what moved.
- Pin and gate swapping where the netlist permits (FPGA pin permutation, MCU pin
  muxing, memory byte lanes). On PCBs, `pcb::swap`: groups of equivalent pins from the caller,
  such as a byte lane's DQ bits or a microcontroller's GPIOs, and the two pins of passives from
  `kicad::swap_groups`, swapped where that shortens their nets, and written back by
  `kicad::replace_pad_nets` with a list of the pads changed for the schematic. On FPGAs, the
  router already takes any pin of a cluster's equivalent input and output classes, as VPR does,
  and `fpga::rotate_luts_for_timing` moves each LUT's latest inputs to its fastest pins after
  routing, which VPR does not do, with `fpga::write_rotated_netlist` writing the `.net` for VPR.

**Exit:** measurable completion or quality gains over the one-pass flow on the
hardest benchmarks; small netlist changes cause small layout changes.

On FPGAs the first half is met ([results](docs/benchmarks.md#fpga-placement-and-routing-in-a-loop)).
On VTR's 20 MCNC circuits and 19 Verilog benchmarks, each with three seeds, placing again where
the router found congestion took the narrowest channel that `guide`'s router needs 4.2% to 5.5%
below the one-pass flow's on MCNC, and 5.7% to 8.3% on the Verilog suite, depending on the seed;
with VPR's router, 0.6% to 2.1% and 4.8% to 6.6%, which puts the Verilog suite 7.5% to 9.4% below
VPR's own placement, from 1.6% to 3.0%, with critical paths within 0.4% and as much wire or less.
mkSMAdapter4B, 16% wider than VPR's placement before, is now as narrow. Every placement and route
passed VPR's checks. On MCNC, rotating LUT inputs after routing took critical paths 24% shorter by
VPR's analysis of the same routes, on `guide`'s placements and on VPR's alike; the Verilog
architecture's LUT inputs are all as fast. Left for later: blob_merge, which still needs 62
tracks against VPR's 58 with VPR's router; VPR's measures of the loop on LU32PEEng and mcml, which
`guide`'s router routes at least 2 tracks narrower; a loop that runs as fast as one pass; and
permuting fracturable LEs to free outputs, and memories' address bits.

The second half is met ([results](docs/benchmarks.md#engineering-changes)). On ten of VTR's
circuits, with 1% and 5% of the nets changed, incremental routing kept 99% and 94% of the routes,
where routing from scratch kept 54% and 43%, and with 1% and 5% of the blocks moved, 94% and 84%
against 52% and 39%: all legal, for 0.2% to 0.9% more wire, a critical path as long within 0.2%,
and 0.4% to 7% of the expansions. With 1% and 5% more blocks, incremental placement moved 20% and
32% of the blocks by about a tile, where placing from scratch moved 96% and 98% by 18 tiles, for
a bounding box 0.3% larger and as much routed wire, within 1%, in a third of the time. On
generated problems, with nets changed or cells blocked, it kept 94% to 99.9% of the routes,
against 60% to 97%. Left for later: FPGA placement's timing and congestion terms in incremental
placement, measured changes to the device on real fabrics, and real engineering changes rather
than random ones.

On PCBs ([results](docs/benchmarks.md#pcb-placement-and-routing-in-a-loop)), the loop routes
every curated board to completion, as the one pass does, and KiCad 10.0.3 finds nothing wrong
with either. It is better than the one pass on five boards of the nine with parts to move, and
by construction never worse by its measure: `TS4231-BOB` from 11 vias and 71 mm to 8 and 64,
`udb-s` from 16 and 143 to 13 and 125, `ddr5-testbed` from 177 and 932 to 170 and 904, as many
vias as by hand, and `lisn` and `lpddr4-testbed` by a via or 6 mm. It gains nothing on
`GB-LIVE32`, `placebo` and `nk3-nrf-mini`: windows of a few parts don't reach what makes the
hand placements cheaper, and the hand placements still route with fewer vias on six boards.
Engineering changes on the routed boards keep every track and via when nothing changed, and
change about the copper the change touches: where routing from scratch changed 31% to 174% of
a board's track for one part or net, incremental routing changed 0% to 53%, with as many
connections or more, in seconds where routing from scratch takes minutes on the large boards. DQ swaps for length undo a careless byte-lane assignment on
`ddr5-testbed`, but make the designers' own assignment worse on both DRAM boards: swaps that
pay need judging by routing. Left for later on PCBs: placement that prices vias, larger
re-placements than windows, swaps judged by routing, and making the loop's rounds cheaper than
the first routing on the small boards.

### M10: More domains and 1.0

- At least one domain outside electronics, as proof that the core is general. **Done:**
  multi-agent path finding, `mapf`, as a time-expanded `RoutingSpace` that the engine
  negotiates unchanged, with an independent checker and a reader for the MovingAI benchmarks.
  On six of their maps, 1500 instances, it solved 1498 within a minute on one thread, LaCAM3
  1500, EECBS 1237 and CBSH2-RTC 308, with the lowest sums of costs of the suboptimal solvers
  nearly everywhere and within 1.7% of the optimum where one is known
  ([results](docs/benchmarks.md#multi-agent-path-finding-on-the-movingai-benchmarks)).
  LaCAM3 is several times faster to a first solution on crowded maps. Left: larger maps,
  where safe-interval path planning would likely be needed, the even scenarios, and the
  tracker's best known solutions.
- ASIC global routing against the ISPD benchmarks (below).
- `nextpnr`/FPGA Interchange and IPC-2581 interop as demand dictates.
- Command-line tool. **Done:** `guide`, behind the `cli` feature (`cargo install guide
  --features cli`), with no dependency: `guide <domain> <action>` for `fpga route|place|flow`,
  `pcb route|place|flow|drc`, `dsn route` (a drop-in for Freerouting in KiCad's DSN workflow),
  `mapf solve` and `asic route|score`, each with `--help`, a JSON report, one meaning for exit
  statuses, and a time limit where the engine can be cancelled
  ([README](README.md#command-line-tool)). PCB routing has its own: `pcb::route_with` takes a
  deadline, a cancel flag and a progress callback, `RouteOptions::work_budget` bounds a run
  reproducibly, and a run stopped by any of them returns what it has routed, with each
  connection left open and its cause (`pcb route` and `dsn route` take `--time-limit`,
  `--work-budget` and `--seed`). Left: a time limit or cancellation for the placers, the PCB
  flow and ASIC routing, which have no cancel flag yet; Ctrl-C handling, which needs a signal
  handler std doesn't have; and progress within a run for the other domains.
- API review and stabilization. **Started:** the [review](docs/api-review.md) of all 405 public
  items, with the consistent fixes made (option and result types `#[non_exhaustive]`, `# Errors`
  and `# Panics` everywhere, `missing_docs` denied, examples per domain, the feature each item
  needs on docs.rs, errors that give their sources, and a test of the API's guarantees), and a
  [draft semver and determinism policy](docs/stability.md). Left: the decisions both list,
  including what determinism promises across versions.

**Exit (1.0):** published benchmark results in at least three domains; a documented,
stable public API; a semver policy that covers result determinism.

ASIC global routing ([benchmarks](docs/benchmarks.md#asic-global-routing-against-the-ispd-contests)):
the `asic` feature reads the ISPD 2007 and 2008 contests' designs, routes them with the engine,
pattern routes and layer assignment, writes their route files and scores them as the contest's
own script does, which agrees with `eval2008.pl` on every route file both have scored. All 12 of
the ISPD 2008 designs that the best routers route free of overflow are free of it, with 0.9%
more wirelength than the contest's winner, 2.2% more than NCTU-GR 2.0 and 1.0% more than
FastRoute 4.1, by geometric mean, in seconds to 16 minutes on 12 threads. What took the last
three there: crowding that rises steeply as an edge fills, nets routed longest first, and
negotiation that starts again from its best routes, with what it has learned, when it stalls;
and for wirelength, Steiner trees for pattern routes, Z shapes, a pass that takes back detours,
and layers that leave room for the nets after. On the four that no router solves, it is within
the contest's range on bigblue4 and newblue4 and behind its best on newblue3 and newblue7, in 24
minutes to 1.7 hours where the first results took up to 8. Left for later: those four, where the
published routers are ahead, and the CPU time, which steep crowding raised on the designs that
converged before, to up to 8.6 times SGR's on one thread.

## Not yet scheduled

Ideas worth keeping, without a place in the plan yet: learned heuristics for net
ordering or congestion prediction; GPU acceleration for analytic placement;
any-angle and topological (rubber-band) PCB routing; 3D and flex/rigid-flex boards;
WebAssembly builds; bindings for other languages.
