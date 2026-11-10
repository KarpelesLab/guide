# Curated KiCad boards

Real open-source boards for testing `src/kicad` and the clearance check in `src/geom/drc.rs`
against KiCad's own DRC. They are data, and nothing here is ever executed. Each directory
holds the boards of one project, unmodified, with that project's licence.

| Directory | Board | Source | Commit | Licence |
| --- | --- | --- | --- | --- |
| `gekkio` | `GB-BRK-LINK-A`, `GB-LIVE32` | [Gekkio/gb-hardware](https://github.com/Gekkio/gb-hardware), `GB-BRK-LINK-A/` and `GB-LIVE32/` | `b5ade559bd988fd6c4f5941615d4d6d79dec9098` | CC-BY-4.0, by Joonas Javanainen (Gekkio) |
| `dotbots` | `TS4231-BOB` | [DotBots/TS4231-breakout-board](https://github.com/DotBots/TS4231-breakout-board) | `6436410953391c6dbaaeab28109faae5ce8cf74a` | BSD-3-Clause, © Inria |
| `tem-cell` | `lisn` | [PetteriAimonen/tem-cell](https://github.com/PetteriAimonen/tem-cell), `lisn/` | `bb4b3718cbd0cc8b6eada2540d336a4c0132440d` | CC-BY-4.0: © 2023 Petteri Aimonen. Licensed under CC-BY-4.0. https://essentialscrap.com/ |
| `placebo` | `placebo` | [dotcypress/placebo](https://github.com/dotcypress/placebo) | `c5da1b37fa3adf2bd180505f552df92d45fefc87` | MIT or Apache-2.0, at your option, © 2021-2023 Vitaly Domnikov |
| `bugs` | `flea` | [jimmerricks/bugs](https://github.com/jimmerricks/bugs), `pcbs/flea/` | `b7523af2a10d2c6054e7aaba1ee52f7c13fb3587` | MIT, © 2021 Jim Merricks White |
| `nitrokey` | `nk3-nrf-mini` | [Nitrokey/nitrokey-3a-mini-nrf52-hardware](https://github.com/Nitrokey/nitrokey-3a-mini-nrf52-hardware) | `7f36ec7eec0d1cccd23da7a3dccc96c6083c08a0` | CERN-OHL-S-2.0, by Nitrokey |
| `udb-s` | `udb-s` | [Unified-Daughterboard/UDB-S](https://github.com/Unified-Daughterboard/UDB-S) | `278cd5cab1120cc2634130fda4c8c02b82570c73` | MIT, © 2023 Unified Daughterboard Project |
| `starshipraider` | `isi-channel` | [azonenberg/starshipraider](https://github.com/azonenberg/starshipraider), `boards/phy-fixtures/isi-channel/` | `93b2d0d19ce35fa745e3cf6889a3ea70b23e41a1` | BSD-3-Clause, © 2016-2021 Andrew Zonenberg |
| `lpddr4-testbed` | `lpddr4-testbed` | [antmicro/lpddr4-testbed](https://github.com/antmicro/lpddr4-testbed) | `93e67cfc3f267443d9b637073688457e8f5c1f91` | Apache-2.0, © 2020-2025 Antmicro |
| `ddr5-testbed` | `ddr5-testbed` | [antmicro/ddr5-testbed](https://github.com/antmicro/ddr5-testbed) | `5db9286b45b5ad9b44f2d82230609038476f95b9` | Apache-2.0, © 2022-2025 Antmicro |

Each board is its `.kicad_pcb` and its `.kicad_pro`, as in the source. The footprints in them
come partly from KiCad's own libraries, whose licence, CC-BY-SA-4.0 with an exception for
designs made with them, puts no terms of its own on the boards.

They were chosen to vary: two to six layers; through-hole, fine-pitch and wafer-level parts;
custom, trapezoid, oval and rounded pads, and pads with clearances of their own; arc tracks,
from 72 on `udb-s` to 544 on `isi-channel`; zones, teardrops, differential pairs,
net classes, and pads and footprints turned by other than right angles. They span file formats
from an early KiCad 6 (`20210925`) to KiCad 9 (`20241229`).

The last two are the dense ones, for M6: each is a single DRAM chip in a BGA, broken out to
the 260 contacts of a SO-DIMM edge, on six layers with 0.1 mm tracks and clearances and vias
of 0.15 mm drills. `lpddr4-testbed` has a 200-ball LPDDR4 at 0.8 by 0.65 mm pitch, one channel
of which is wired, and `ddr5-testbed` an x8 DDR5 of 82 balls at 0.8 mm. They are the largest
files here, about 5 MB each, mostly the fabrication drawings of their footprints, and are kept
unmodified too.

## What KiCad found

The `*.kicad-drc` files beside each board are not from the source projects: they record what
`kicad-cli pcb drc` from KiCad 10.0.3 finds on the board, under the board's own rules (`own`)
or with every net class's clearance raised (`plus-0.1mm` and so on), which makes many more
findings out of the same copper. Each line is a kind of finding and the UUIDs of its two items.
Lines starting with `arc` are findings on arcs that KiCad misses, but makes once those arcs are
split into short segments.

`src/kicad/curated_tests.rs` compares our check with these. To check them against KiCad, and to
write them again:

```text
GUIDE_KICAD=1 cargo test --release --features kicad curated            # compare live
GUIDE_KICAD=1 GUIDE_KICAD_UPDATE=1 cargo test --release --features kicad curated  # rewrite
```

[`docs/benchmarks.md`](../../docs/benchmarks.md#pcb-clearance-checks-against-kicad) has the
results.

## DSN files

The `*.dsn` files beside each board are not from the source projects either: they are derived
from the boards, as their licences allow, by KiCad 10.0.3's own Specctra exporter
(`pcbnew.ExportSpecctraDSN`, as "Export Specctra DSN" does for Freerouting), and are under
the boards' licences. Two changes were made for it: the first line, which names the file, is cut from an
absolute path to the file's name, and on `nk3-nrf-mini`, whose three mounting holes share the
reference `REF**`, which the exporter refuses, two of them were renamed `REF**_2` and `REF**_3`
first, in memory. `src/dsn/curated_tests.rs` reads them and compares them with the boards, and
imports sessions back into KiCad:

```text
cargo test --release --all-features curated_dsn                                # compare
GUIDE_KICAD=1 cargo test --release --all-features -- curated_dsn curated_sessions  # with KiCad
GUIDE_KICAD=1 GUIDE_KICAD_UPDATE=1 cargo test --release --all-features curated_dsn_files  # rewrite
```

[`docs/benchmarks.md`](../../docs/benchmarks.md#pcb-dsn-designs-against-kicad-boards) has the
results.

## Routing them

`src/kicad/routing_tests.rs` takes each board's tracks, vias and zone fills out, routes its
nets again with `pcb::route`, and checks the result with our DRC and for connectivity. With
`GUIDE_KICAD=1` it also writes each routed board, by `kicad::replace_routing`, and runs
`kicad-cli pcb drc` on it:

```text
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_KICAD=1 cargo test --release --features kicad curated_boards_route -- --nocapture
```

[`docs/benchmarks.md`](../../docs/benchmarks.md#pcb-routing-of-the-curated-boards) has the
results.

## Routing them with Freerouting

[`scripts/freerouting/compare.sh`](../../scripts/freerouting/compare.sh) strips each board with
KiCad's `pcbnew` as the routing test does, exports its DSN, routes it with Freerouting 2.5.0,
imports the session into the stripped board, routes the same stripped board with `pcb::route`,
and measures and checks both alike, KiCad's DRC included. Nothing it makes is kept here.

```text
JAVA=jre/bin/java FREEROUTING_JAR=freerouting-2.5.0.jar scripts/freerouting/compare.sh OUT_DIR
```

[`docs/benchmarks.md`](../../docs/benchmarks.md#pcb-routing-against-freerouting) has the
results.
