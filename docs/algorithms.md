# Algorithms

Candidate algorithms and where each applies. This is a survey to guide
implementation, **not a list of what is implemented**, though notes say where something has
been built. Inclusion means "worth evaluating"; benchmarks decide what stays.

## Point-to-point search

| Algorithm | Use | Notes |
| --- | --- | --- |
| Lee maze routing | Grids | The classic: complete and optimal, but explores too much on its own. Useful as a test oracle. |
| Dijkstra, A\* | All domains | The default workhorse. Tight lower bounds matter more than anything else here. |
| Line probe (Mikami-Tabuchi, Hightower) | Sparse PCB layouts | Finds few-bend routes quickly, without guaranteeing them optimal. |
| Theta\* | Any-angle routing | Path shortening during search. A later option for PCB. |
| Lookahead tables | FPGA | Precomputed per-wire-type costs give A\* a much tighter bound on real fabrics. Widely used in VPR and nextpnr-style routers. Built as `fpga::MapLookahead`: 40 times fewer expansions on VTR's MCNC circuits, with incremental rerouting (see `docs/benchmarks.md`). |

## Multi-terminal nets

| Algorithm | Use | Notes |
| --- | --- | --- |
| Incremental tree growth | All | Route the nearest sink, then search again from the whole partial tree. Simple and a good baseline. |
| KMB Steiner approximation | Graphs | Metric-closure based, with a bounded approximation ratio. |
| Hanan grid, FLUTE | Rectilinear | Lookup-table-based near-optimal trees, very fast. A strong fit for global routing. |
| Timing-driven trees | FPGA, ASIC | Trade wirelength for source-to-critical-sink delay. |
| Topology-constrained trees | PCB | Daisy chain, star and T are specified rather than optimized. |

## Multi-net routing

| Algorithm | Use | Notes |
| --- | --- | --- |
| Negotiated congestion (PathFinder) | All | Sharing is initially allowed, then priced up until conflicts resolve. **The core algorithm of this project.** |
| Rip-up and reroute | All | Remove conflicting nets and try again in a new order. Negotiation is a principled form of this. |
| Push and shove | PCB detailed | Move existing tracks aside to make room, as in KiCad's interactive router. Crucial for dense boards. |
| Network-flow escape routing | BGA fanout | Exact for ordered and unordered pin escape in many cases. |
| Multi-commodity flow, LP relaxation | Global routing | Strong bounds on small or coarse problems. |
| Layer assignment | PCB, ASIC | Project to 2D, route, then assign layers while minimizing vias. |
| Conflict-based search | Multi-agent | Optimal search over conflicts. Possibly interesting for small, very dense PCB regions. |

## Tree-dependent (Elmore) delays: a design, not yet built

Today each step has a fixed delay (`DelaySpace::step_delays`), and a connection's delay is the
sum along the tree. VPR's delay is an Elmore delay over the route tree: a node's delay depends
on the capacitance downstream of it, which grows as branches are added. Behind an unbuffered
switch, it also depends on the resistance back to the last buffer. The fixed delay is exact on
a fully buffered device for a tree that does not branch. Otherwise it is too low, by the
internal capacitance of the switches a branch adds and by what unbuffered switches pass on.

**Why it was not built now.** Inside a search, an Elmore delay is not a property of a step: a
branch added later changes the delay of sinks connected earlier. A search that priced it would
no longer be a shortest path over fixed costs, and would lose its lower bound and optimality. So
the delay used to *route* stays fixed per step, as in VPR, whose searches also use a per-node
estimate and only compute the tree's Elmore delay after a net is routed. What can change is the
delay used to *analyse*. That needs RC data, which only the `fpga` domain has.

**The design.**

1. A trait beside `DelaySpace`, say `TreeDelaySpace`, with one method that takes a net's
   finished route and fills the delay from the root to each of its nodes. A default computes the
   sum of step delays, which is today's behaviour, so that the engine needs one code path.
2. `RrGraph` implements it from what it already reads: each node's `R` and `C`, and each
   switch's `R`, `Cin`, `Cout`, `Cinternal`, `Tdel` and whether it is buffered. Two passes over
   the tree, as VPR's `load_new_subtree_R_upstream`, `load_new_subtree_C_downstream` and
   `load_route_tree_Tdel` do. First, downstream capacitance from the leaves up, stopping at
   buffers. Second, upstream resistance and delay from the root down. Both are linear in the
   size of the tree. All of it is `f64` multiplication and addition in a fixed order, so it is
   deterministic, and it ends in picoseconds rounded as `RrGraph::step_delay` rounds them.
3. A delay-model option on `TimingDriven`: fixed per step (the default, as now), or the tree's.
   With the tree's, the engine replaces `Worker::arrival` after a net is routed with what the
   trait computes, and that is what goes into `TimingOutcome::connection_delays`, the timing
   graph and the criticalities. Searches keep the fixed step delays and their bound.
4. Fixed routes and non-configurable sets fit as they are. A fixed route's delays come from the
   same trait. The members of a tied set are nodes of the tree, so their capacitance is counted
   downstream, which is exactly the load the fixed delay misses.

**What to measure before making it the default.** How far the fixed and tree delays disagree
on real VTR graphs, by critical path and per connection. Whether criticalities from tree
delays change routes enough to matter. What the two passes cost per iteration, since they run
for every rerouted net. And the validator would need an independent implementation of the same
formulas to check reported delays, as the timing tests already do for fixed delays.

## Post-processing

Via removal, corner smoothing, track pulling (rubber-banding), length tuning through
meanders, and rerouting of critical connections.

On PCBs, `pcb` does two of these after routing, each checked against the design rules as
routing is: via removal, by routing each net with vias again at ten times the price of a via
and keeping the result if it has fewer, and track pulling, by replacing each run of tracks
between pads, vias and branches with the longest octilinear shortcuts that are legal.

Two more come after those. Differential pairs are routed again, side by side, in place of
the routes their two nets were given: an A* search for the pair's centreline, whose nodes
carry the direction they were reached in so that it turns by 45° at most and runs straight
around a layer change, checking a track as wide as both tracks and their gap, which is then
offset into the two tracks with mitred corners. The pair is kept only if it connects what the
two routes did, so it never costs another net its route. Then length tuning: trombone
meanders on the straight tracks of nets shorter than their constraints ask, as high as there
is room for, the last just high enough to land on the length, and for a pair, meanders of
both tracks together, which add the same length to both.

## Placement

| Algorithm | Use | Notes |
| --- | --- | --- |
| Simulated annealing | All | Simple and strong on small and medium problems; the VPR baseline, and a natural fit for mixed constraints. Scales poorly alone. Built as `place::Annealer`, with VPR's adaptive schedule. |
| Min-cut partitioning (Fiduccia-Mattheyses) | Clustering, seeding | Divide large netlists; also useful for grouping PCB functional blocks. |
| Quadratic and analytic placement (SimPL, HeAP) | Large FPGA | Solve a relaxed problem, then spread and legalize. Built as `place::place_analytic`, after SimPL with bound-to-bound net models. |
| Electrostatic placement (ePlace, DREAMPlace) | Very large | State of the art for wirelength at scale. Heavier machinery; GPU optional. |
| Force-directed | PCB | Natural for free-space placement with few items. |
| Legalization (Tetris, Abacus, min-cost flow) | All discrete | Snaps a global solution to legal sites with minimum displacement. Tetris-like greedy legalization built as `place::legalize`, for sites and free space, with rows after Abacus in free space. |
| Detailed placement | All | Local window swaps after legalization. Built as low-temperature annealing over a few steps. FastPlace-DP's global swap was tried and gained nothing on top of it. |
| Timing-driven weighting | FPGA, ASIC | Re-weights critical connections between iterations. Built for FPGAs as VPR's timing cost, criticalities from a timing analysis each temperature. |
| RUDY congestion estimate | All | Spreads each net's wire evenly over its bounding box. Built as `place::CongestionGrid::rudy`, priced by `place::CongestionCost`. |

### What `place` does

Generic placement (roadmap M7) is in `src/place`, measured in
[`docs/benchmarks.md`](benchmarks.md#placement-on-synthetic-problems).

- **Model.** Items go on typed sites, one each, or in free space, where outlines may not overlap.
  Items can be fixed, limited to a rectangle, turned by quarter turns, and tied into rigid macros.
  A placement's cost is a sum over nets of a `CostTerm`, by default weighted half-perimeter
  wirelength, kept per net so that a move only prices the nets it touches.
- **Annealing.** VPR's schedule: the first temperature is 20 standard deviations of random moves'
  cost changes, each temperature tries `inner_num * units^(4/3)` moves, the temperature falls by
  0.5 to 0.8 depending on the fraction accepted, and the range limit, counted in columns and rows
  of the item's kind of site or in pitches, follows that fraction towards 44%; the fraction is of
  the moves priced, not counting those abandoned as illegal. Moves are moves to empty places,
  swaps, quarter turns, median moves as in VPR, which send an item into the median region of its
  nets, and macro shifts that send the single items they land on to the nearest sites they leave
  and push a macro they land on back the other way, which lets macros of one shape trade places.
  Checked against VPR's published schedule (Betz and Rose, 1997), the temperature factors, the
  range limit's update, the first temperature and the exit test are VPR's; only the rate is
  taken over the moves priced rather than all moves tried.
  The acceptance test uses `math::exp`.
- **Analytic.** Bound-to-bound springs (Spindler, Schlichtmann and Johannes, 2008), whose
  quadratic cost equals half-perimeter wirelength at the positions they were built from, solved
  per axis by conjugate gradients with a Jacobi preconditioner, written here with a fixed order of
  operations. Spreading by recursive bisection makes an upper bound, each further solve is
  anchored to it with growing weight, as in SimPL (Kim, Lee and Markov, 2012), until the bounds
  are within 10%. A quadratic placement of an unanchored netlist collapses to a point; the
  first spreading then decides its shape, which on grids happened to be the right one.
- **Legalization.** Tetris-like: macros from the bottom up, then items from left to right, each to
  the nearest free spot by Manhattan distance, searched in rings. Items limited to a region go
  first. In free space, rows after Abacus (Spindler, Schlichtmann and Johannes, 2008) are tried
  too, along x and along y, as tall as the tallest unit and cut by fixed items: units in order of
  their targets go to the row where they move least, or fill rows in turn where space is tight,
  and units that would overlap in a row merge into clusters placed at their mean target, in
  integers on the pitch lattice. The shortest legal result wins. On sites, spreading already
  leaves the global placement legal on every benchmark, so rows there would gain nothing. When
  everything leaves gaps that later units can't use, it packs instead (Hill's Tetris, then
  bottom-left packing in free space), which is legal but ignores the targets.
- **Congestion feedback.** `CongestionEstimate` is the narrow interface: an overflow, in
  thousandths, for a rectangle. `CongestionGrid` implements it from RUDY (Spindler and Johannes,
  2007) or from demand and capacity a router fills in, and `CongestionCost` adds a net's
  wirelength times that overflow to its cost. Closing the loop with the global router is M9.
- **Known optima.** Generators build problems whose reference placement meets a per-net lower
  bound, after Peko (Chang, Cong and Xie, 2003): grids, chains, Peko netlists with pads and typed
  columns, and squares in free space.

### FPGA placement

FPGA placement (roadmap M8) is in `src/fpga/placer.rs`, on top of `src/place`, and measured
against VPR in [`docs/benchmarks.md`](benchmarks.md#fpga-placement-against-vpr).

- **Problem.** `fpga::build_place_problem` makes a `place::PlacementProblem` from a packed
  netlist and the device of a routing resource graph: a site per position of each tile, typed by
  the tile, pins on the tile of the block that VPR's `pin_width_offset` and `pin_height_offset`
  put them on, carry chains as macros from the architecture's `<directlist>`, as VPR's
  `alloc_and_load_placement_macros` finds them, and fixed blocks from a `.place` file, as with
  VPR's `--fix_clusters`. Clock and constant nets are left out, as VPR leaves them out.
- **Cost.** VPR's, for `criticality_timing`: each net's bounding box counted in tiles, times the
  crossing factor for its size (Cheng, 1994), and each connection's delay times its criticality
  to an exponent that grows from 1 to 8 as the range limit shrinks. Each temperature the
  placement is timed again with every connection at the delay model's delay, and both terms are
  divided by their totals so that the cost stays near 1, as VPR does, through
  `place::Annealer::set_cost`.
- **Delay model.** A table by columns and rows apart, after VPR's `delta` model: the least delay
  from any `SOURCE` of a tile of the most common type, from five places, to any `SINK` at each
  offset, by Dijkstra on the graph's own step delays, leaving out carry chains' direct
  connections.
- **Flow.** Analytic global placement and legalization by plain wirelength, then annealing that
  starts cold, at a twentieth of the moves' standard deviation over 8 steps, which is how VPR 9
  starts too, from a placement that puts blocks near their neighbours' centroid at
  `std_dev / 64`. Annealing from a random placement, hot, as VPR did before, is the other method.

### PCB placement

PCB placement (roadmap M8) is in `src/pcb/place.rs`, on top of `src/place`, with KiCad's side in
`src/kicad/place.rs`, and measured on the curated boards in
[`docs/benchmarks.md`](benchmarks.md#pcb-placement-of-the-curated-boards).

- **Problem.** `kicad::placement_design` makes a `pcb::place::PlacementDesign` of a board: a
  component for each footprint, whose room is its courtyard's box (`F.CrtYd` or `B.CrtYd`),
  grown to its pads where they reach past it, or failing a courtyard, the box of its pads, copper
  and silkscreen grown by a quarter of a millimetre; its pins are its pads with nets, at their
  real offsets. Footprints that go through the board take room on both sides. What stays fixed
  is decided by `kicad::fixed_because`: locked, without pads, not assembled (`board_only`, or
  left out of both the bill of materials and the position files: test and touch pads, card
  edges), drawing the board's edge, a mounting hole, connector, fiducial, switch or battery by
  its reference or library, or with a courtyard that reaches the board's edge. On boards whose
  project makes silkscreen overlaps errors, silkscreen takes room too.
- **Free region.** The board is rasterized on 100 µm cells: blocked outside the edge (by
  crossings of each row with the edge's lines), within the edge clearance and twice the spacing
  of it, and under fixed parts, keepouts, copper graphics and text on copper, grown by the
  spacing. A fixed area array, a BGA, also blocks the other side under it, for its fan-out vias.
  Blocked cells merge into rectangles, which are fixed items of a `place::PlacementProblem` in
  free space, so the generic placers need nothing new.
- **Sides.** `src/place` knows one plane, so the board is placed in passes: parts through the
  board first, against both sides, with the rest movable but taking no room; then the front,
  the back's parts still taking no room, so that they sit where their nets want; then the back,
  legalized from there, the front fixed; then the front again, the back fixed. Where a side's
  parts would take more than 60% of its free room, surface-mount parts, those of two pins and
  the smallest first, go to the other side while it has room, and `kicad::replace_placement`
  flips them as KiCad does.
- **Cost.** Half-perimeter wirelength on pin positions, with nets of more than 12 pins at a
  quarter weight. A pin of a net that a copper zone connects is drawn into the box of its
  zones, as three-pin nets with the box's corners fixed, unless the zones cover most of the
  board, rather than to the net's other pins. A capacitor between a supply and a ground, by
  their names, gets a net to a power pin on the same supply of an IC, the supply's capacitors
  spread over its power pins in order, which costs four times as much beyond 2 mm. Thermal
  groups, given by the caller, are nets of their own. After the first placement, a RUDY map of
  the signal nets against half of what the layers carry at the board's track pitch prices nets
  through congested bins, as `place::CongestionCost`, in annealing at a low temperature.
- **Room for routing.** Outlines grow by 0.25 mm a side, 0.35 mm more for parts of 8 pins or
  more, never less than half the largest clearance. If that leaves no room, the spacing goes down
  by quarters, to none.
- **Writing back.** `kicad::replace_placement` edits the board's text where footprints moved:
  the footprint's `(at)`, its pads', texts' and fields' angles, which a board file holds with
  the footprint's own included, its zones' points, which it holds on the board, and for a flip,
  every point mirrored, layers swapped, chamfers and texts' justification mirrored. Routing goes,
  as it was for the old placement. On boards where silkscreen overlaps are errors, a moved
  footprint's text that would overlap silkscreen or pads goes beside its outline, or is hidden.

## Support structures

| Structure | Use |
| --- | --- |
| Corner stitching | Plane layouts with fast neighbour and free-space queries |
| R-tree or uniform grid | Clearance queries; benchmarks will decide which, per domain |
| Constrained Delaunay triangulation | Topological routing, navigation meshes |
| Compressed sparse row graphs | Compact FPGA routing graphs |
| Radix, bucket or 4-ary heaps | Priority queues on integer keys, including `f64` costs bit-cast to `u64` |
| Static timing analysis | Delay and criticality; incremental updates |

## References

- C. Y. Lee, "An Algorithm for Path Connections and Its Applications", *IRE Trans.
  Electronic Computers*, 1961.
- P. Hart, N. Nilsson, B. Raphael, "A Formal Basis for the Heuristic Determination of
  Minimum Cost Paths", *IEEE Trans. Systems Science and Cybernetics*, 1968.
- L. Kou, G. Markowsky, L. Berman, "A Fast Algorithm for Steiner Trees", *Acta
  Informatica*, 1981.
- C. Fiduccia, R. Mattheyses, "A Linear-Time Heuristic for Improving Network
  Partitions", *DAC*, 1982.
- S. Kirkpatrick, C. Gelatt, M. Vecchi, "Optimization by Simulated Annealing",
  *Science*, 1983.
- J. Ousterhout, "Corner Stitching: A Data-Structuring Technique for VLSI Layout
  Tools", *IEEE TCAD*, 1984.
- R. Nair, "A Simple Yet Effective Technique for Global Wiring", *IEEE TCAD*, 1987.
- C.-E. Cheng, "RISA: Accurate and Efficient Placement Routability Modeling", *ICCAD*, 1994.
  The crossing factor of VPR's bounding-box cost.
- L. McMurchie, C. Ebeling, "PathFinder: A Negotiation-Based Performance-Driven
  Router for FPGAs", *FPGA*, 1995.
- V. Betz, J. Rose, "VPR: A New Packing, Placement and Routing Tool for FPGA
  Research", *FPL*, 1997.
- D. Hill, "Method and System for High Speed Detailed Placement of Cells Within an
  Integrated Circuit Design", US patent 6,370,673, 2002. Tetris legalization.
- J. Cong, M. Romesis, M. Xie, "Optimality and Scalability Study of Partitioning and Placement
  Algorithms", *ISPD*, 2003; and C.-C. Chang, J. Cong, M. Xie, "Optimality and Scalability Study
  of Existing Placement Algorithms", *ASP-DAC*, 2003. The Peko benchmarks.
- M. Pan, N. Viswanathan, C. Chu, "An Efficient and Effective Detailed Placement Algorithm",
  *ICCAD*, 2005. FastPlace-DP's global swap.
- P. Spindler, F. Johannes, "Fast and Accurate Routing Demand Estimation for Efficient
  Routability-Driven Placement", *DATE*, 2007. RUDY.
- C. Chu, Y.-C. Wong, "FLUTE: Fast Lookup Table Based Rectilinear Steiner Minimal
  Tree Algorithm for VLSI Design", *IEEE TCAD*, 2008.
- P. Spindler, U. Schlichtmann, F. Johannes, "Abacus: Fast Legalization of Standard
  Cell Circuits with Minimal Movement", *ISPD*, 2008.
- P. Spindler, U. Schlichtmann, F. Johannes, "Kraftwerk2: A Fast Force-Directed Quadratic
  Placement Approach Using an Accurate Net Model", *IEEE TCAD*, 2008. The bound-to-bound model.
- K. Daniel, A. Nash, S. Koenig, A. Felner, "Theta\*: Any-Angle Path Planning on
  Grids", *JAIR*, 2010.
- M. Gort, J. Anderson, "Analytical Placement for Heterogeneous FPGAs", *FPL*, 2012.
- M.-C. Kim, D.-J. Lee, I. Markov, "SimPL: An Effective Placement Algorithm", *IEEE TCAD*,
  2012.
- J. Lu et al., "ePlace: Electrostatics-Based Placement Using Fast Fourier Transform
  and Nesterov's Method", *ACM TODAES*, 2015.
- G. Sharon, R. Stern, A. Felner, N. Sturtevant, "Conflict-Based Search for Optimal
  Multi-Agent Pathfinding", *Artificial Intelligence*, 2015.
- Y. Lin et al., "DREAMPlace: Deep Learning Toolkit-Enabled GPU Acceleration for Modern
  VLSI Placement", *DAC*, 2019.
- G. Li, Y. Ding, Y. Xie, "Tackling the Qubit Mapping Problem for NISQ-Era Quantum
  Devices", *ASPLOS*, 2019.
- K. Murray et al., "VTR 8: High-Performance CAD and Customizable FPGA Architecture
  Modelling", *ACM TRETS*, 2020.
