# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.1](https://github.com/KarpelesLab/guide/compare/v0.0.0...v0.0.1) - 2026-11-04

### Added

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

- *(space)* default the grid diagonal to a cost whose sums cannot round
- *(space)* keep the exact grid bound for all costs whose sums cannot round

### Other

- add stubs for timing analysis and the fpga feature
- *(engine)* check engine results with the independent validator
- mark milestone M1 as done
- *(search)* verify optimality end to end and add throughput benchmarks
- note the driver check and allow non-step resource queries
- *(space)* update graph tests for merged edges
- align numeric types and geometry plan with consumers
- add README badges
