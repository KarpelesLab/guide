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

Each board is its `.kicad_pcb` and its `.kicad_pro`, as in the source. The footprints in them
come partly from KiCad's own libraries, whose licence, CC-BY-SA-4.0 with an exception for
designs made with them, puts no terms of its own on the boards.

They were chosen to vary: two to four layers; through-hole, fine-pitch and wafer-level parts;
custom, trapezoid, oval and rounded pads, and pads with clearances of their own; arc tracks,
from 72 on `udb-s` to 544 on `isi-channel`; zones, teardrops, differential pairs,
net classes, and pads and footprints turned by other than right angles. They span file formats
from an early KiCad 6 (`20210925`) to KiCad 8 (`20240108`).

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
