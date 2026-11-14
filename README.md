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

> **Status: pre-alpha.** The API is not stable yet and changes between 0.0.x releases.
> Routing and placement work for FPGAs and PCBs, measured against VPR, KiCad and
> Freerouting in [docs/benchmarks.md](docs/benchmarks.md), and the same engine solves
> multi-agent path finding, measured against CBSH2-RTC, EECBS and LaCAM3, and ASIC global
> routing, against the ISPD contests' published results. See the
> [roadmap](ROADMAP.md) for what is done and what is next.

## Features

All optional, so the default build has no dependencies:

| Feature | Adds |
| --- | --- |
| `fpga` | VTR's routing resource graphs, netlists, placements and architectures; `.route` and `.place` output |
| `pcb` | The geometry layer, clearance checks, zone filling, PCB routing and placement |
| `kicad` | Reading KiCad boards and rules, and writing routes, fills and placements back (implies `pcb`) |
| `dsn` | Specctra DSN import and SES export (implies `pcb`) |
| `mapf` | Multi-agent path finding on graphs and grids, and the MovingAI benchmarks |
| `asic` | ASIC global routing: the ISPD 2007 and 2008 contests' designs and route files, and their scoring |
| `cli` | The `guide` command-line tool, below; enables all of the above |

## Command-line tool

```sh
cargo install guide --features cli
```

`guide <domain> <action> [options] <files>` runs each domain on its field's own files:

| Command | Does |
| --- | --- |
| `guide pcb route board.kicad_pcb -o routed.kicad_pcb` | Routes what a KiCad board's tracks don't connect yet (`--strip`: everything), fills its zones, and writes the board back. The `.kicad_pro` beside it gives the rules |
| `guide pcb place board.kicad_pcb -o placed.kicad_pcb` | Places the footprints that aren't locked or fixed |
| `guide pcb flow board.kicad_pcb -o done.kicad_pcb` | Places and routes, then places again where routing found trouble |
| `guide pcb drc board.kicad_pcb` | Checks clearances, holes, keepouts, mask bridges and connectivity, of a `.kicad_pcb` or a `.dsn` |
| `guide dsn route board.dsn -o board.ses` | Routes a Specctra design into a session, in place of Freerouting: export the DSN from KiCad, import the session back. `-de`/`-do` are accepted, as Freerouting's |
| `guide fpga route --arch arch.xml --rr-graph rr.xml --net c.net --place c.place -o c.route` | Timing-driven routing of a VPR design, to a `.route` file VPR loads |
| `guide fpga place --arch arch.xml --rr-graph rr.xml --net c.net -o c.place` | Placement of a packed VPR netlist |
| `guide fpga flow --arch arch.xml --rr-graph rr.xml --net c.net -o c.place --route-output c.route` | Both, placing again around what routing overused |
| `guide mapf solve room.map room.scen --agents 100 -o plans.txt` | Multi-agent paths on a MovingAI map, checked, written as LaCAM writes them |
| `guide asic route design.gr -o design.route` | ISPD 2007/2008 global routing, scored as `eval2008.pl` scores it; `guide asic score` scores any router's routes |

Every command takes `--help`, `--json FILE` for a machine-readable report (`-` for standard
output), `-q` and `-v`; those that use them take `--seed`, `--threads` (`-j`) and
`--time-limit SECONDS`, after which routing stops with the best result so far. Results are the
same for any thread count. PCB routing uses one thread, so `pcb route` and `dsn route` take no
`--threads`; they take `--seed` (another order of the nets, for a second attempt),
`--time-limit`, and `--work-budget N`, a number of search nodes for the whole run, which stops
it as a time limit does but gives the same result on any machine. The exit status is 0 for a
complete and legal result, 1 for an incomplete or illegal one (nets unrouted, overuse,
violations), and 2 for a wrong command line or an input that can't be read. There is no signal
handler, so Ctrl-C ends a run at once, without writing anything: use `--time-limit` to stop
early and keep the result.

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
| [Benchmarks](docs/benchmarks.md) | Measured results, against VPR, KiCad and Freerouting |
| [API review](docs/api-review.md) | The public API's inventory, what was made consistent, and what is left before 1.0 |
| [Stability](docs/stability.md) | Draft semver and determinism policy, awaiting a decision |

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
