# Placing reticle's designs with guide

A standalone crate that takes a design through [reticle](../../../../reticle)'s own FPGA flow
up to placement, places it either with reticle's placer or with guide's, and then routes it
with reticle's router and runs the checks reticle's `fpga --bitstream` makes. Results are in
`docs/benchmarks.md`, "Placing reticle's designs".

It is not part of guide's package (`scripts/` is excluded) and not a workspace member. reticle
is expected beside guide's checkout, at `../reticle` from guide's root. reticle is only read:
nothing here changes it.

```sh
export CARGO_TARGET_DIR=/tmp/reticle-place
cargo build --release
cd ../reticle      # reticle's checkout: the paths below are its own
$CARGO_TARGET_DIR/release/reticle-place run --device ecp5-12f-CABGA256 \
    --placer guide --seed 1 --ip ip,testdata/fpga/cynthion \
    --constraints testdata/fpga/cynthion/usb_host_target.rcf \
    testdata/fpga/cynthion/usb_host_target.v
```

`run` prints one line of `key=value` pairs: the netlist's size, the placement's time and
reticle's wirelength (`hpwl`, in tiles, every signal), the route's passes, pips and nodes,
and `ok=yes` when every check passed, or `failed=<stage>` and the reason on stderr.
`--placer reticle` places with `reticle::fpga::place::place` at the options `reticle fpga`
gives the family; `--placer guide` (the default) with guide. `--ip` lists directories whose
`.v` files supply modules the sources name and do not declare, as `reticle build` finds a
library's. `inspect` prints a device's site kinds, one tile of each type, and the netlist.

Fabric databases are reticle's: `reticle fetch prjtrellis-db` and `prjxray-db`, found in
`~/.cache/reticle` or through `RETICLE_TRELLISDB` and `RETICLE_CHIPDB`.

## Families

- **ECP5** (`ecp5-12f-CABGA256`, Project Trellis): the flow of `reticle fpga --bitstream`
  from the database to the bitstream it would write, with every check it makes: the cells
  configured, block RAMs that do not overlap, every clock on a global network, no bit an arc
  needs clear set, and every set bit decoding back to exactly the arcs the router chose.
- **iCE40** (`ice40-hx1k-tq144`, reticle's synthetic architecture): `fpga::place_and_route`,
  with the bitstream generated as the check.
- **7 series** (`xc7a35t-cpg236`, Project X-Ray): the flow of `reticle fpga --bitstream` at its
  defaults up to the bitstream, which is not written: lookup tables given the inputs the
  router reached them on, every sink walked back, no shorts, clocks on the network, and each
  domain's latest path by reticle's fitted delay model (`path_<domain>=<ns>/<period>`).

## How guide's problem is built

`src/model.rs`: a site per graph site, at its tile scaled by four with a slice's sites a step
apart, so that a carry chain moving slice by slice is a rigid macro; pins fixed by the
constraints; macros from dedicated wiring as reticle finds it; and reticle's site rules as
guide's shared resources (`place::Shared`): pins of two sites on one wire carry one signal,
sites that exclude each other hold one cell, a pool of a tile's wires carries as many signals
as it has wires, with a bel's output a source that carries only its own, a way through a
lookup table shut by a cell on it, an 18-bit block RAM's neighbour left empty, and a clock
buffer kept to the sites its pad reaches.
