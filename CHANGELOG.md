# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
