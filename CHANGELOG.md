# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.17](https://github.com/KarpelesLab/guide/compare/v0.0.16...v0.0.17) - 2026-11-17

### Added

- *(pcb)* a negotiation for every net that its share ends in near its end goes on once, and what it was measured to do
- *(pcb)* every net can be routed against every other at once before the passes, on one grid (RouteOptions::whole, off by default)

### Fixed

- *(pcb)* a net that a negotiation for every net ended with in pieces is not taken out whole when its turn finds no way for the rest

### Other

- *(benchmarks)* a net at the width its tracks have, tried, and where a pass on six layers spends its time

## [0.0.16](https://github.com/KarpelesLab/guide/compare/v0.0.15...v0.0.16) - 2026-11-17

### Added

- *(pcb)* a host that fills its zones gives the spokes of its reliefs
- *(pcb)* the room of a thermal relief's spokes can cost other nets' tracks, and what the board keeps apart is found once and told

### Fixed

- *(pcb)* what completes a net puts no via beside one the net has

### Other

- *(benchmarks)* the spokes as the host's fill makes them, measured

## [0.0.15](https://github.com/KarpelesLab/guide/compare/v0.0.14...v0.0.15) - 2026-11-17

### Added

- *(pcb)* two nets that cross in a row of fine pitch have the crossing planned on another layer where the row's has no room, and cleanup takes out the hops between two vias

## [0.0.14](https://github.com/KarpelesLab/guide/compare/v0.0.13...v0.0.14) - 2026-11-17

### Added

- *(pcb)* a run goes on from an earlier run's routes and may move them, and a place with several pads open can have a negotiation of its own

## [0.0.13](https://github.com/KarpelesLab/guide/compare/v0.0.12...v0.0.13) - 2026-11-17

### Added

- *(pcb)* a route that keeps what it reaches begins another tree from a terminal it leaves out

### Other

- *(benchmarks)* what else was tried for PCB routing negotiated, and what sweep-v2.2's nets are shut in by

## [0.0.12](https://github.com/KarpelesLab/guide/compare/v0.0.11...v0.0.12) - 2026-11-17

### Added

- *(pcb)* a run that settles negotiates for each open connection before its chains
- *(asic)* a first try at mild crowding, and overflow priced at what it adds
- *(scripts)* place reticle's designs with guide, route them with reticle
- *(place)* shared resources, macro slots and a target density
- *(scripts)* every MovingAI map, even scenarios, the tracker's best known, bounded runs
- *(mapf)* negotiate over safe intervals, with a first solution by LaCAM
- *(scripts)* route reticle's timing-driven designs by reticle's model exactly
- *(engine)* reticle's guards on critical connections, and criticality against the latest path

### Fixed

- *(asic)* raise layer prices only where another layer of the edge has room
- *(scripts)* exact tiles for guide's columns, clock regions, forbidden sites

### Other

- *(benchmarks)* ISPD third round, CPU time and newblue3
- *(asic)* cheaper bounds and cell numbers on the plane
- *(benchmarks)* placing reticle's designs, against reticle's placer
- *(scripts)* tables for reticle placement runs
- *(benchmarks)* every MovingAI map, even scenarios, and the tracker's best known
- *(benchmarks)* reticle's timing-driven designs, routed by reticle's model

## [0.0.11](https://github.com/KarpelesLab/guide/compare/v0.0.10...v0.0.11) - 2026-11-17

### Added

- *(scripts)* route reticle's designs with guide and with reticle's router
- *(engine)* aim each search of a tree at its nearest terminals

### Fixed

- *(pcb)* the chains of a run that settles are for connections, not for the pads cut off their fills

### Other

- *(benchmarks)* routing reticle's designs against reticle's router

## [0.0.10](https://github.com/KarpelesLab/guide/compare/v0.0.9...v0.0.10) - 2026-11-17

### Added

- *(pcb)* a run with a budget settles what rip-up leaves open, by chains kept only if the board has more for them
- *(cli)* --time-limit, --work-budget and -v progress for every long command
- run control for the placers, the flows, ASIC routing and MAPF

### Other

- run control everywhere, and why there is no Ctrl-C handler
- *(roadmap)* mark M9 done, to keep improving

## [0.0.9](https://github.com/KarpelesLab/guide/compare/v0.0.8...v0.0.9) - 2026-11-16

### Fixed

- *(pcb)* a track's end and a via at one point join the layers from either side

### Other

- a via on a via, found

## [0.0.8](https://github.com/KarpelesLab/guide/compare/v0.0.7...v0.0.8) - 2026-11-16

### Added

- *(pcb)* a net that waited keeps what it reaches before it crosses anything
- *(pcb)* a run with a budget joins the pads that its routes cut off the planned fills
- *(pcb)* a run with a budget plans the ways out of a fine row for the whole row

### Fixed

- *(pcb)* the ways out of fine rows are planned only if asked
- *(pcb)* two tracks side by side in a rule area are necked by the area's clearance

### Other

- the pads that routes cut off their fill for good, what was tried, and whose they were
- PCB routing with a second wait, measured, and what was not kept

## [0.0.7](https://github.com/KarpelesLab/guide/compare/v0.0.6...v0.0.7) - 2026-11-16

### Fixed

- *(pcb)* a run with a budget pulls every net tight before it looks for fewer vias
- *(pcb)* a pad is said to be shut in at the width its net was routed at

### Other

- PCB routing planned sooner, measured, and what was not kept
- *(pcb)* Routed::failed is final only after a run that ended on its own
- *(pcb)* a run with a budget ends global routing once it gains nothing
- *(pcb)* a host's fill is cut round new copper in a window, not opened whole
- *(geom)* the parts of a polygon of many vertices are worked out once
- *(geom)* the fixed items of a board are packed in its index once they are in
- *(pcb)* the steps from a node are asked of the world in one query

## [0.0.6](https://github.com/KarpelesLab/guide/compare/v0.0.5...v0.0.6) - 2026-11-16

### Added

- *(pcb)* a run with a budget moves a net aside before it takes one out
- *(pcb)* a run with a budget plans the ways out of fine rows and deep balls
- *(pcb)* Routed::failed, the open nets whose own search found no way

### Fixed

- *(pcb)* a host's fill of few edges is cut only where copper is too close
- *(pcb)* a work budget that is not spent stops no run with zones to join

### Other

- PCB routing with ways out planned, measured, and what was not kept

## [0.0.5](https://github.com/KarpelesLab/guide/compare/v0.0.4...v0.0.5) - 2026-11-14

### Added

- *(pcb)* a passage as wide as a track between two pads has a lattice of its own
- *(pcb)* a budget is shared out between routing, cleanup and the joining of zones

### Other

- PCB routing in parts, measured, and what was not kept
- *(pcb)* the nodes of a net's searches are numbered by block of its lattice
- *(pcb)* a fill of many edges is asked about by its parts

## [0.0.4](https://github.com/KarpelesLab/guide/compare/v0.0.3...v0.0.4) - 2026-11-14

### Added

- *(pcb)* RouteOptions::fanout_via, a smaller via for the escapes of trapped pads
- *(pcb)* RouteOptions::fills, for a host that fills its zones itself

### Fixed

- *(pcb)* a net with a plane is taken out whole in a run with no budget, in every pass
- *(pcb)* only a narrow zone of a net of few pads is a pour
- *(pcb)* a pour between a few pads is copper, not a plane to cut through

### Other

- PCB routing with a budget, measured, and what was not kept
- *(pcb)* what route_all returns has a name
- *(pcb)* a run with a budget routes for the most connections soonest
- *(pcb)* a host's fill of many edges is measured to by its parts

## [0.0.3](https://github.com/KarpelesLab/guide/compare/v0.0.2...v0.0.3) - 2026-11-14

### Added

- *(pcb)* RunControl::fill_on_stop, for a run back within a second of its deadline
- *(pcb)* a net too wide to leave its pads is routed narrower
- *(cli)* pcb route and dsn route take --no-vias-in-pads
- *(pcb)* a deadline or a cancel flag stops global routing too
- *(cli)* pcb route and dsn route take a time limit, a work budget and a seed
- *(pcb)* a seed for another order of the nets
- *(pcb)* a routing error names the track or via at fault
- *(pcb)* a net that cannot be routed whole keeps its tree, with keep_partial
- *(pcb)* run control for routing, and the connections a run leaves open
- *(pcb)* pcb::edges, a board's edge from its outline, and Zones::set_pad
- *(geom)* [**breaking**] RuleSpec::areas, a clearance and a track width inside an area
- *(geom)* [**breaking**] net ties, the items at which nets may touch
- *(geom)* [**breaking**] Pad::stack, a pad's other shapes on other layers
- *(pcb)* [**breaking**] the layers that a run's and a net's tracks may be on
- *(geom)* [**breaking**] Keepout::except, the nets that a keepout lets in
- *(pcb)* [**breaking**] zones that are planes or fixed copper, by zone and by how much of its net a zone covers
- *(pcb)* [**breaking**] zones filled by the host, and a zone's number of spokes and their direction
- *(geom)* Shape::rings and Shape::outline, to read a shape back
- *(pcb)* RouteOptions::vias_in_pads, to keep vias out of pads with no hole

### Fixed

- *(pcb)* a region's way out of its cluster is checked again when it is placed
- *(pcb)* a stopped run goes back to the routes that made the most connections
- *(pcb)* a pair's other ends tried when the first can't be coupled
- *(pcb)* meanders as low as fit, where a net is short of its length
- *(pcb)* a pair's tracks 2 nm over its gap, not 16

### Other

- PCB routing after the release candidate, measured, and what was not kept
- *(kicad)* GUIDE_VIAS_IN_PADS=0 routes the curated boards with vias kept out of pads
- *(pcb)* a fixed zone of many edges goes into the world as parts of a few
- *(pcb)* rip-up takes only the branches in the way out of a net of very many pads
- *(pcb)* cleanup's search for fewer vias goes further for nets of few pads
- PCB routing searched from both ends, measured, and what was not kept
- *(pcb)* a node budget for cleanup's routes with fewer vias
- *(pcb)* one tree through a net's searches, looked at from both ends
- how PCB routing is stopped, and which commands take which limits
- the roadmap, with what a host's board may say
- loops clippy 1.100 flags as needless_range_loop

## [0.0.2](https://github.com/KarpelesLab/guide/compare/v0.0.1...v0.0.2) - 2026-11-11

### Added

- *(place)* Debug for Annealer, as the routing engine and search have
- errors that wrap another give it as their source, and a test of API guarantees
- [**breaking**] mark search, PCB, ASIC and MAPF results #[non_exhaustive]
- *(asic)* let negotiation run 15 iterations without progress before it restarts
- *(asic)* converge the hardest ISPD 2008 designs, with less wirelength
- *(engine)* crowding exponent, and ripping up chosen nets
- *(cli)* the guide command-line tool, behind the cli feature
- *(pcb)* make a routing design of a Specctra DSN board with Design::from_dsn
- *(asic)* global routing of the ISPD 2007 and 2008 contests' designs
- *(engine)* price resources as they fill up, with EngineOptions::crowding
- *(mapf)* shorten legal solutions an agent at a time, and tune the negotiation
- *(mapf)* multi-agent path finding over a time-expanded space, by the negotiated-congestion engine
- *(fpga)* let the loop route again from the routes of the round before
- *(fpga)* rotate LUT inputs for timing after routing
- *(fpga)* place and route in a loop, with the router's congestion fed back
- *(kicad)* place and route in a loop, write swapped pad nets
- *(pcb)* reroute incrementally, place again in windows, swap pins
- *(fpga)* carry placements and routes over to a changed netlist
- *(engine)* route again from earlier routes after an engineering change
- *(place)* place incrementally after a change, with a displacement term
- *(fpga)* place for congestion by default
- *(fpga)* price channel congestion in FPGA placement
- *(place)* let analytic placement inflate items where routing is congested
- *(pcb)* place a board's footprints, from KiCad and back
- *(pcb)* move pairs routed first in rip-up, and check solder mask bridges
- *(pcb)* complete supply nets as tracks, and pairs first across layers
- *(pcb)* give a net its meander room back from its neighbours
- *(fpga)* tune the negotiation on both VTR suites
- *(engine)* give up early on runs predicted not to converge
- *(pcb)* route the DRAM boards completely, with pairs first if asked
- *(kicad)* read track widths and pairs' gaps from custom rules
- *(fpga)* place packed netlists on VPR's devices
- *(place)* let an annealer's cost term change between temperatures
- *(pcb)* route nets with zones through them
- *(pcb)* fill copper zones, as KiCad's zone filler does
- *(scripts)* tables for hard blocks and setup-aware own timing
- *(scripts)* compare with VPR on VTR's Verilog benchmarks
- *(place)* legalize by rows in free space, and anneal with median moves
- *(pcb)* meander nets to lengths and skews, with KiCad's length rules
- *(pcb)* route differential pairs side by side
- *(place)* pack when greedy legalization runs out of room, and reach macros further
- *(place)* generic placement, by annealing and by analytic placement
- *(pcb)* escape routing from fine-pitch and area-array footprints
- *(pcb)* a second, finer pass for nets left unrouted, and defaults from measuring
- *(kicad)* write routes back into a board, and route the curated boards
- *(pcb)* route boards over the geometry layer, globally then in detail
- *(fpga)* tables of a VPR comparison can show an earlier run beside it
- *(fpga)* lookahead options in vtr_route, and its time in the tables
- *(fpga)* a map lookahead for RrGraph, and options that use it
- *(engine)* a bound weight, and incremental rerouting of nets
- *(search)* weighted bounds, and no queueing for a bound of the maximum
- *(dsn)* read Specctra DSN designs and write SES sessions
- *(fpga)* a harness to route VPR's designs and compare with VPR
- *(fpga)* RrGraph::engine_options, with history in the graph's units

### Fixed

- *(mapf)* take LaCAM3's sum of costs, not its sum of loss, and run it with threads too
- *(geom)* build cleanly with each PCB feature alone, and say what guide does
- *(kicad)* keep footprints out of the board's own mask openings
- *(fpga)* route VTR's flagship architecture as VPR does
- *(kicad)* keep new vias to the board's minimum hole, diameter and ring
- *(geom)* report a track or via over a hole when there is no hole clearance
- *(fpga)* leave a logic block through one output pin, as VPR requires

### Other

- API review, and a draft semver and determinism policy
- no powi in results-affecting code
- crate and module examples, and the feature each item needs on docs.rs
- # Errors and # Panics sections, and #[must_use] on methods returning Self
- *(pcb)* [**breaking**] rename pcb::Options to pcb::RouteOptions
- all 12 solvable ISPD 2008 designs free of overflow, with 0.9% more wirelength than the winner
- *(asic)* a net goes on the emptier of two layers that take as many vias
- leave the benchmark scripts out of the published crate
- the command-line tool in the README, architecture, domains and roadmap
- ASIC global routing against the ISPD 2007 and 2008 contests
- *(asic)* score the 2007 3D designs from the 2008 routes, and tabulate against published results
- check the mapf feature alone too
- multi-agent path finding on the MovingAI benchmarks, against CBSH2-RTC, EECBS and LaCAM3
- *(mapf)* scripts to fetch the MovingAI benchmarks, build reference solvers, and compare
- *(mapf)* hold goals through a chain of stays rather than one step to the horizon
- FPGA placement and routing in a loop, and LUT rotation, against the one-pass flow
- *(vtr)* tables of the loop and of LUT rotation against the one-pass flow
- optimise test builds lightly
- PCB placement and routing in a loop, engineering changes, pin swaps
- engineering changes, routed and placed again incrementally
- congestion-aware FPGA placement against VPR, with seeds and weights
- *(fpga)* say what channel_demand counts without linking a private item
- PCB placement of the curated boards, and M8 is done
- ddr5-testbed meets its constraints, and M6 is done
- threads on VPR's graphs, before and after, and where the time went
- *(engine)* make batches pay on FPGA graphs
- DRAM constraints and Freerouting again after supplies and pairs
- republish both VPR comparisons with the tuned negotiation
- DRAM boards routed, and Freerouting compared again on eleven boards
- *(kicad)* curate two DRAM boards, an LPDDR4 and a DDR5 breakout
- compare FPGA placement with VPR's, and mark M8's FPGA half done
- *(fpga)* place designs on small devices, and print the placement tables
- zones and planes on the curated boards
- compare with VPR on VTR's Verilog benchmarks
- *(place)* measure placement before and after, and mark M7 done
- differential pairs and lengths on the curated boards
- *(place)* benchmark placement against known optima, and record M7
- *(roadmap)* mark M5 done, with nk3-nrf-mini's /LED_R left to revisit
- bring the Freerouting comparison and M5 up to date with escape routing
- nk3-nrf-mini with escape routing, and the router's speedups
- *(pcb)* keep via verdicts while a net's own vias come and go
- *(pcb)* cache step verdicts and give up early on shut-in terminals
- *(pcb)* index arcs by their edges and resolve clearances once
- publish the first results of routing against Freerouting
- *(kicad)* route the curated boards with Freerouting and guide alike
- settle what a resource is on a PCB and how routing levels compose
- FPGA routing against VPR before and after the map lookahead
- record the DSN and SES comparisons with KiCad, and Freerouting's use
- publish FPGA routing results against VPR, and mark M3 done

## [0.0.1](https://github.com/KarpelesLab/guide/compare/v0.0.0...v0.0.1) - 2026-11-05

### Added

- *(kicad)* read KiCad boards and their rules for clearance checks
- *(geom)* check zones, own clearances, pairs of nets and hole clearances
- *(timing)* time each clock domain against its own constraint
- *(fpga)* read delays and primitive models from VTR architecture files
- *(engine)* keep improving timing after the first legal result
- *(engine)* take non-configurable edge sets as a whole
- *(engine)* keep fixed and partially pre-routed nets
- *(fpga)* build timing graphs from packed netlists
- *(geom)* add the geometry layer behind a pcb feature
- *(engine)* route timing-driven, with per-connection criticality
- *(fpga)* give routing resource graphs step delays and a delay bound
- *(space)* add DelaySpace and TimedGraph for step delays
- *(engine)* add deterministic parallel routing
- *(space)* add dense resource slots and node membership
- *(fpga)* import VTR routing graphs, netlists and placements
- *(timing)* add static timing analysis
- *(steiner)* add directed KMB tree construction
- *(engine)* add negotiated congestion router
- *(search)* add target costs and limits to path search
- *(validate)* add independent routing result checker
- *(space)* settle step identity, start node resources and net drivers
- *(search)* add multi-source A* with reusable state
- *(space)* add grid and explicit graph routing spaces
- *(rng)* add seeded generator and platform-independent exp
- *(net)* add nets, terminals and routes
- *(core)* add costs, identifiers and the routing space trait

### Fixed

- *(ci)* check out test data without line-ending conversion
- *(space)* default the grid diagonal to a cost whose sums cannot round
- *(space)* keep the exact grid bound for all costs whose sums cannot round

### Other

- record the KiCad clearance comparison, and mark M4 done
- *(kicad)* cross-check the clearance check with KiCad on curated boards
- *(algorithms)* design tree-dependent Elmore delays for timing analysis
- *(roadmap)* mark M2 done
- *(fpga)* route mult_4x4 timing-driven from VTR files
- *(geom)* benchmark the R-tree against a uniform grid
- *(benchmarks)* measure timing-driven routing on generated fabrics
- *(engine)* correct option claims against the reproducible sweep
- describe the batches that determinism actually relies on
- *(engine)* check parallel results with the independent validator
- *(engine)* add reproducible engine benchmarks and option sweep
- say what the architecture notes already cover
- *(fpga)* tidy the licensing of the VTR samples
- add stubs for timing analysis and the fpga feature
- *(engine)* check engine results with the independent validator
- mark milestone M1 as done
- *(search)* verify optimality end to end and add throughput benchmarks
- note the driver check and allow non-step resource queries
- *(space)* update graph tests for merged edges
- align numeric types and geometry plan with consumers
- add README badges
