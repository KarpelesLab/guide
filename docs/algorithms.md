# Algorithms

Candidate algorithms and where each applies. This is a survey to guide
implementation, **not a list of what is implemented** (at this stage, nothing is).
Inclusion means "worth evaluating"; benchmarks decide what stays.

## Point-to-point search

| Algorithm | Use | Notes |
| --- | --- | --- |
| Lee maze routing | Grids | The classic: complete and optimal, but explores too much on its own. Useful as a test oracle. |
| Dijkstra, A\* | All domains | The default workhorse. Tight lower bounds matter more than anything else here. |
| Line probe (Mikami-Tabuchi, Hightower) | Sparse PCB layouts | Finds few-bend routes quickly, without guaranteeing them optimal. |
| Theta\* | Any-angle routing | Path shortening during search. A later option for PCB. |
| Lookahead tables | FPGA | Precomputed per-wire-type costs give A\* a much tighter bound on real fabrics. Widely used in VPR and nextpnr-style routers. |

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

## Post-processing

Via removal, corner smoothing, track pulling (rubber-banding), length tuning through
meanders, and rerouting of critical connections.

## Placement

| Algorithm | Use | Notes |
| --- | --- | --- |
| Simulated annealing | All | Simple and strong on small and medium problems; the VPR baseline, and a natural fit for mixed constraints. Scales poorly alone. |
| Min-cut partitioning (Fiduccia-Mattheyses) | Clustering, seeding | Divide large netlists; also useful for grouping PCB functional blocks. |
| Quadratic and analytic placement (HeAP) | Large FPGA | Solve a relaxed problem, then spread and legalize. |
| Electrostatic placement (ePlace, DREAMPlace) | Very large | State of the art for wirelength at scale. Heavier machinery; GPU optional. |
| Force-directed | PCB | Natural for free-space placement with few items. |
| Legalization (Tetris, Abacus, min-cost flow) | All discrete | Snaps a global solution to legal sites with minimum displacement. |
| Detailed placement | All | Local window swaps after legalization. |
| Timing-driven weighting | FPGA, ASIC | Re-weights critical connections between iterations. |

## Support structures

| Structure | Use |
| --- | --- |
| Corner stitching | Plane layouts with fast neighbour and free-space queries |
| R-tree or uniform grid | Clearance queries; benchmarks will decide which, per domain |
| Constrained Delaunay triangulation | Topological routing, navigation meshes |
| Compressed sparse row graphs | Compact FPGA routing graphs |
| Radix or bucket heaps | Integer-cost priority queues, often faster than binary heaps |
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
- L. McMurchie, C. Ebeling, "PathFinder: A Negotiation-Based Performance-Driven
  Router for FPGAs", *FPGA*, 1995.
- V. Betz, J. Rose, "VPR: A New Packing, Placement and Routing Tool for FPGA
  Research", *FPL*, 1997.
- C. Chu, Y.-C. Wong, "FLUTE: Fast Lookup Table Based Rectilinear Steiner Minimal
  Tree Algorithm for VLSI Design", *IEEE TCAD*, 2008.
- P. Spindler, U. Schlichtmann, F. Johannes, "Abacus: Fast Legalization of Standard
  Cell Circuits with Minimal Movement", *ISPD*, 2008.
- K. Daniel, A. Nash, S. Koenig, A. Felner, "Theta\*: Any-Angle Path Planning on
  Grids", *JAIR*, 2010.
- M. Gort, J. Anderson, "Analytical Placement for Heterogeneous FPGAs", *FPL*, 2012.
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
