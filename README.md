# guide

[![CI](https://github.com/KarpelesLab/guide/actions/workflows/ci.yml/badge.svg)](https://github.com/KarpelesLab/guide/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/guide.svg)](https://crates.io/crates/guide)
[![docs.rs](https://img.shields.io/docsrs/guide)](https://docs.rs/guide)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A placement and routing engine for Rust, aiming at the best achievable results
across every kind of problem that reduces to *connecting terminals through
contended resources*:

- **PCBs**: fixed or movable components, multiple layers, vias, design rules.
- **FPGAs**: logic placement, fixed I/O, routing over a device's switch fabric.
- **Anything else** where the same algorithms apply: ASIC global routing,
  multi-agent path finding, qubit mapping, harnesses, piping.

> **Status: pre-alpha.** Version 0.0.0 is an empty scaffold. There is no routing
> or placement code yet, and everything below describes intent, not features.
> See the [roadmap](ROADMAP.md).

## Why one engine?

PCB and FPGA tools look unrelated, and almost no code is shared between them
today. Underneath, they solve the same problem: many nets compete for limited
resources, and each net's best path depends on what the others do. Negotiated
congestion, rip-up and reroute, Steiner tree construction, annealing and
analytic placement all apply well beyond the field they were published in.

`guide` puts those algorithms behind a small abstraction, so an improvement made
for one domain is available to the others. What genuinely differs (geometry and
design rules on a PCB, a discrete fabric and timing on an FPGA) stays in domain
layers.

## Goals

- **Result quality first.** Completion, wirelength, vias and timing are measured
  against established tools on public benchmarks. "Best" has to be a number.
- **Deterministic.** The same input, seed and options give the same output, for
  any thread count and on any platform.
- **Verifiable.** Results are checked by a validator that shares no search code
  with the router.
- **Honest about failure.** An unroutable problem yields the best partial result
  plus a report of what failed and why, never a panic or a silent omission.
- **Embeddable.** A library first, with no GUI and no required external solver.
  The crate forbids `unsafe` code.

## Non-goals

Schematic capture, HDL synthesis, bitstream generation, Gerber/CAM output,
GUIs, and electromagnetic field solving. `guide` takes a netlist and a
description of the space to route in, and returns placements and routes. Other
tools handle what comes before and after.

## Documentation

| Document | Contents |
| --- | --- |
| [Roadmap](ROADMAP.md) | Milestones, their exit criteria, and how quality will be measured |
| [Architecture](docs/architecture.md) | Layers, core abstractions, design decisions, open questions |
| [Domains](docs/domains.md) | What each target domain requires and how it maps onto the core |
| [Algorithms](docs/algorithms.md) | Candidate algorithms, where each applies, and references |

## Development

Requires Rust 1.95 or newer (edition 2024).

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

With `kicad-cli` 10.0.3 installed, `GUIDE_KICAD=1 cargo test --release --all-features` also
compares the PCB clearance check with KiCad's own, live, and imports sessions written here into
KiCad, through its `pcbnew` module for Python.

Commits follow [Conventional Commits](https://www.conventionalcommits.org/).
[release-plz](https://release-plz.dev/) derives versions and the changelog from
them: pushing to `master` opens or updates a release PR, and merging that PR
publishes to crates.io.

## License

[MIT](LICENSE)
