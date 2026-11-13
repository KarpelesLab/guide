# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
