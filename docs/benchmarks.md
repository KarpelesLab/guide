# Benchmarks

This is the start of the benchmark report that the [roadmap](../ROADMAP.md) asks for. Most of
it covers the engine, on synthetic instances that this repository generates. Those are
useful for tuning, for catching regressions and for checking what the docs say. They are **not
evidence about real designs**. The comparisons on real designs so far are
[FPGA routing against VPR](#fpga-routing-against-vpr), on VTR's MCNC circuits and
[its Verilog benchmarks](#vtrs-verilog-benchmarks), the
[clearance check against KiCad's](#pcb-clearance-checks-against-kicad), on real boards,
[those boards read from DSN](#pcb-dsn-designs-against-kicad-boards) against the same boards read
from KiCad's files, [PCB routing of the same boards](#pcb-routing-of-the-curated-boards),
checked by KiCad, and [their zones' fills](#pcb-zones-and-planes) against KiCad's. [Placement](#placement-on-synthetic-problems) is measured on synthetic problems
whose optimum is known.

## Running

```text
cargo bench --bench engine                             # criterion timings, about a minute
GUIDE_ENGINE_SWEEP=options cargo bench --bench engine  # option tables, about a minute
GUIDE_ENGINE_SWEEP=refine cargo bench --bench engine   # refinement after negotiation, seconds
GUIDE_ENGINE_SWEEP=expansion cargo bench --bench engine  # time per expansion, about 4 minutes
GUIDE_ENGINE_SWEEP=timing cargo bench --bench engine   # timing-driven routing, about a minute
GUIDE_ENGINE_SWEEP=all cargo bench --bench engine      # all four sweeps
```

```text
cargo bench --bench place                            # placement timings, by criterion
GUIDE_PLACE_SWEEP=quality cargo bench --bench place  # placement against the optimum, a minute
GUIDE_PLACE_SWEEP=threads cargo bench --bench place  # the same placements on 1 and 16 threads
```

The sweeps print Markdown tables and skip criterion. The default bench prints each instance's
counts ahead of its timing. Any other value of `GUIDE_ENGINE_SWEEP` is refused.

The engine's results do not depend on the machine, so everything here except for times can be
compared exactly: a rerun that prints a different solved count, expansion count or cost means
that routing changed. Sweeps run up to 16 instances at once, which changes no result.

## Instances

All come from fixed `guide::rng::Rng` seeds, in `benches/engine.rs`. Capacities are all 1.

| Family | Instances | Space | Nets |
| --- | --- | --- | --- |
| Dense grids, tuning seeds | 72: sizes 24, 32 and 40, seeds 500 to 523 | Square grid of 2 layers | One per row, from the left edge to a shuffled row on the right edge, on layer 0 |
| Dense grids, other seeds | 72: the same sizes, seeds 1000 to 1023 | The same | The same |
| Multi-terminal grid | 72: 28 nets at size 32, 42 at 48, 56 at 64, seeds 700 to 723 | Square grid of 2 layers | A driver and 1 to 4 sinks on distinct random cells of layer 0 |
| Island fabric | 18: 16 x 16 tiles, 5, 6 and 8 tracks, seeds 0 to 5 | Pins and horizontal and vertical wires, with edges both ways at cost 1 | 80 nets of a driver and 1 to 3 sinks on the pins of shuffled tiles |
| Tight island fabric | 6: as above at 4 tracks | The same | The same |
| FPGA-like fabric | 12: 16 x 16 and 32 x 32 tiles, seeds 900 to 905 | The shape in `benches/search.rs`, narrowed to 1 track and 4 pins per site, with 4 connections per output pin and per wire, and 2 per wire to input pins | Up to 200 and 800 nets of a source and 1 to 3 sinks within 6 tiles. Sinks run out, which leaves about 140 and 545 nets |

The tuning seeds are the instances behind the numbers in the docs of `EngineOptions`, recovered
from the throwaway harness: its solved counts and expansions for caps of 50 and 100 match the
tables below. The defaults were chosen by looking at them, so the other seeds are there to see how
far the conclusions carry, on the same shape of problem.

The multi-terminal and FPGA-like instances were sized to be hard but mostly solvable, by trying
a few densities and keeping one close to the limit. That choice was made while looking at the
default options, which is worth bearing in mind when comparing against them.

## Results

Measured on 2026-11-04, on the routing code of commit `ffbff3d` (`src/` unchanged by the commit
that adds this file), with rustc 1.97.1.

### Default options

To compare a rerun against, at a glance:

| Family | Solved | Mean iterations, solved | Iterations, all | Expansions | Cost, solved | Overuse left |
| --- | --- | --- | --- | --- | --- | --- |
| dense grids, tuning seeds | 71/72 | 29.9 | 2221 | 14519721 | 107926.0 | 1 |
| dense grids, other seeds | 71/72 | 26.9 | 2012 | 12939973 | 108200.0 | 1 |
| multi-terminal grid | 67/72 | 24.2 | 2119 | 31756554 | 202979.0 | 5 |
| island fabric | 18/18 | 3.6 | 65 | 639221 | 29255.0 | 0 |
| FPGA-like fabric | 12/12 | 9.4 | 113 | 3703435 | 64253.8 | 0 |

### What the docs of `EngineOptions` claim

The tuning seeds at the default options unless it says otherwise. Costs are compared only over
instances that both settings solved, because a setting that solves only the easier instances
would otherwise look cheap. The harness's averages did not do that.

| Option | Claim | Measured | Verdict |
| --- | --- | --- | --- |
| `max_iterations` | A cap of 100 against 50 solves 71 of 72 against 58 | 71 against 58 | Reproduces, but flatters the change: 71 against 68 on the other seeds |
| `max_iterations` | For 3% more expansions | 2.5% more, and 1.4% on the other seeds | Reproduces |
| `max_iterations` | Twice as long to give up | 1.82 times the expansions on the tight island fabric, where nothing was solved at any cap | Roughly reproduces |
| `first_present` | 0.5 takes 30% to 65% of the expansions of 0 | 42% to 62% by grid size, 43% to 53% on the other seeds, 28% to 55% on the island fabric | Reproduces, 28% being just outside. Does not extend: 69% to 82% on multi-terminal grids, 93% on the FPGA-like fabric |
| `first_present` | Solves at least as many | 71 against 58, and 71 against 49 on the other seeds. Equal on every other family | Reproduces overall. One group goes the other way: 22 against 23 at multi-terminal size 48 |
| `first_present` | Routes about 4% cheaper | 3.7% and 3.3% on dense grids, 2.5% on the island fabric, 2.2% on multi-terminal grids, 1.2% on the FPGA-like fabric | Reproduces on dense grids only |
| `present_growth` | 1.3 takes about 1.5 times the expansions | 1.21 times, and 1.36 on the other seeds, with 1.51 only for one size of those | **Does not reproduce** |
| `present_growth` | 1.3 solves fewer | 66 against 71, and 64 against 71 on the other seeds | Reproduces on dense grids. **Reverses on multi-terminal grids**: 71 against 67 |
| `history_factor` | 0.2 and 5 both do worse | 57 and 41 solved against 71, at 1.51 and 2.74 times the expansions, and much the same on the other seeds | Reproduces |
| `reroute_all` | Rescues one problem of 72 | **At the default cap it solves 67 against 71**, four fewer. At a cap of 200, 72 against 71, but 71 against 72 on the other seeds | **Does not reproduce** at the default cap. Rescues 4 on multi-terminal grids |
| `reroute_all` | Routes no better | 0.4% cheaper among instances both solved, and 0.1% dearer on the other seeds | Reproduces |
| `reroute_all` | About twice the expansions | 1.83 times, 1.91 on the other seeds, 1.58 to 2.21 on other families | Reproduces |
| `shuffle_seed` | Rescues one problem of 72 | Seed 9, the one tried, gets 72. Seeds 1, 2 and 3 get 70, 67 and 68, and on the other seeds all four get 65 or 66 | Reproduces for that seed only. **Misleading**: shuffling usually lost problems on dense grids |
| `shuffle_seed` | Iterations and expansions about the same or a little worse | Mean iterations about the same on dense grids, and up to 1.9 times on the FPGA-like fabric. Expansions 5% to 28% more on dense grids, 2% to 8% on the island fabric, 15% to 24% on multi-terminal grids, 8% to 45% on the FPGA-like fabric | **Understated** on dense grids. Fair on the island fabric |
| `shuffle_seed` | Routes up to 1% dearer | Up to 1.1% on dense grids, 0.3% on the island fabric, 1.8% on multi-terminal grids, 5.8% on the FPGA-like fabric | Reproduces for the families it was measured on. Does not extend |

The figure of 1.5 for `present_growth` came from an earlier sweep with a `first_present` of 0, of
12 instances per problem size, which saw ratios of 1.13 to 1.71 and was not repeated after that
default changed. That setting is not the whole explanation. Here, with `first_present` at 0, the
ratio is still 1.20 on the tuning seeds and 1.26 on the others.

The result that matters most for reading any of this is how much a solved count moves for no
good reason. Changing only the shuffle seed moves the dense grids between 65 and 72 of 72, and
the multi-terminal grids between 67 and 72. So a difference of a few problems out of 72 says
little by itself, and that includes several of the claims above.

### Option sweep on the tuning seeds

One option varied at a time from the defaults. "By size" is for 24, 32 and 40 in that order.
"Cost / default" is over the instances solved by both, which "Solved by both" counts.

#### `max_iterations`

| Value | Solved | By size | Mean iterations, solved | Expansions | / default | By size | Cost / default | Solved by both | Overuse left |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 25 | 34/72 | 14+15+5 | 13.9 | 12958267 | 0.892 | 0.887 0.968 0.864 | 1.000 | 34 | 60 |
| 50 | 58/72 | 20+22+16 | 21.9 | 14170211 | 0.976 | 0.964 0.997 0.970 | 1.000 | 58 | 16 |
| 100 (default) | 71/72 | 23+24+24 | 29.9 | 14519721 | 1.000 | 1.000 1.000 1.000 | 1.000 | 71 | 1 |
| 200 | 71/72 | 23+24+24 | 29.9 | 14605393 | 1.006 | 1.056 1.000 1.000 | 1.000 | 71 | 1 |
| 400 | 72/72 | 24+24+24 | 32.8 | 14638165 | 1.008 | 1.077 1.000 1.000 | 1.000 | 71 | 0 |

#### `first_present`

| Value | Solved | By size | Mean iterations, solved | Expansions | / default | By size | Cost / default | Solved by both | Overuse left |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0.0 | 58/72 | 23+18+17 | 49.7 | 30053470 | 2.070 | 1.606 2.406 2.016 | 1.038 | 57 | 23 |
| 0.1 | 68/72 | 24+23+21 | 31.8 | 15386437 | 1.060 | 1.103 1.216 0.992 | 1.007 | 67 | 4 |
| 0.25 | 70/72 | 24+23+23 | 31.8 | 14692695 | 1.012 | 0.992 1.079 0.989 | 1.001 | 69 | 3 |
| 0.5 (default) | 71/72 | 23+24+24 | 29.9 | 14519721 | 1.000 | 1.000 1.000 1.000 | 1.000 | 71 | 1 |
| 1.0 | 70/72 | 24+23+23 | 33.1 | 15365216 | 1.058 | 1.114 1.116 1.027 | 1.000 | 69 | 2 |
| 2.0 | 65/72 | 23+20+22 | 30.1 | 14685083 | 1.011 | 1.121 1.213 0.915 | 1.004 | 64 | 12 |

#### `initial_present`

| Value | Solved | By size | Mean iterations, solved | Expansions | / default | By size | Cost / default | Solved by both | Overuse left |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0.1 | 60/72 | 22+20+18 | 42.1 | 20423164 | 1.407 | 1.597 1.786 1.227 | 1.010 | 59 | 18 |
| 0.25 | 61/72 | 20+22+19 | 33.1 | 15770000 | 1.086 | 1.423 1.184 0.993 | 1.003 | 60 | 11 |
| 0.5 (default) | 71/72 | 23+24+24 | 29.9 | 14519721 | 1.000 | 1.000 1.000 1.000 | 1.000 | 71 | 1 |
| 1.0 | 67/72 | 23+23+21 | 29.5 | 15453045 | 1.064 | 0.854 1.141 1.069 | 1.000 | 66 | 5 |
| 2.0 | 70/72 | 24+24+22 | 28.5 | 14372159 | 0.990 | 0.777 1.084 0.988 | 1.006 | 69 | 2 |

#### `present_growth`

| Value | Solved | By size | Mean iterations, solved | Expansions | / default | By size | Cost / default | Solved by both | Overuse left |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1.1 | 47/72 | 20+16+11 | 50.0 | 37125815 | 2.557 | 2.746 3.012 2.349 | 0.996 | 46 | 38 |
| 1.3 | 66/72 | 22+24+20 | 34.7 | 17502461 | 1.205 | 1.269 1.393 1.122 | 0.993 | 65 | 6 |
| 1.5 | 69/72 | 23+22+24 | 30.2 | 15266323 | 1.051 | 1.043 1.275 0.966 | 0.996 | 68 | 3 |
| 1.8 (default) | 71/72 | 23+24+24 | 29.9 | 14519721 | 1.000 | 1.000 1.000 1.000 | 1.000 | 71 | 1 |
| 2.2 | 71/72 | 23+24+24 | 31.6 | 14812949 | 1.020 | 1.034 0.990 1.030 | 1.003 | 70 | 1 |
| 3.0 | 71/72 | 23+24+24 | 26.9 | 13891990 | 0.957 | 0.941 1.139 0.888 | 1.008 | 70 | 1 |
| 1.8, first_present 0 | 58/72 | 23+18+17 | 49.7 | 30053470 | 2.070 | 1.606 2.406 2.016 | 1.038 | 57 | 23 |
| 1.3, first_present 0 | 53/72 | 22+18+13 | 50.9 | 35970799 | 2.477 | 2.463 2.872 2.326 | 1.020 | 52 | 30 |

#### `history_factor`

| Value | Solved | By size | Mean iterations, solved | Expansions | / default | By size | Cost / default | Solved by both | Overuse left |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0.0 | 0/72 | 0+0+0 | 0.0 | 82283585 | 5.667 | 5.814 6.723 5.232 | - | 0 | 717 |
| 0.2 | 57/72 | 21+20+16 | 37.0 | 21951206 | 1.512 | 1.568 1.530 1.495 | 0.996 | 56 | 22 |
| 0.5 | 65/72 | 22+23+20 | 33.1 | 17554648 | 1.209 | 0.946 1.244 1.239 | 0.993 | 64 | 7 |
| 1.0 (default) | 71/72 | 23+24+24 | 29.9 | 14519721 | 1.000 | 1.000 1.000 1.000 | 1.000 | 71 | 1 |
| 2.0 | 63/72 | 20+22+21 | 34.7 | 18846667 | 1.298 | 1.452 1.428 1.222 | 1.017 | 62 | 14 |
| 5.0 | 41/72 | 19+13+9 | 55.8 | 39851286 | 2.745 | 2.987 3.403 2.448 | 1.065 | 40 | 87 |

#### `reroute_all`

| Value | Solved | By size | Mean iterations, solved | Expansions | / default | By size | Cost / default | Solved by both | Overuse left |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| false (default) | 71/72 | 23+24+24 | 29.9 | 14519721 | 1.000 | 1.000 1.000 1.000 | 1.000 | 71 | 1 |
| true | 67/72 | 22+23+22 | 28.4 | 26610164 | 1.833 | 1.730 1.821 1.854 | 0.996 | 66 | 5 |
| false, cap 200 | 71/72 | 23+24+24 | 29.9 | 14605393 | 1.006 | 1.056 1.000 1.000 | 1.000 | 71 | 1 |
| true, cap 200 | 72/72 | 24+24+24 | 35.8 | 27605597 | 1.901 | 1.833 1.856 1.930 | 0.996 | 71 | 0 |

#### `shuffle_seed`

| Value | Solved | By size | Mean iterations, solved | Expansions | / default | By size | Cost / default | Solved by both | Overuse left |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| None (default) | 71/72 | 23+24+24 | 29.9 | 14519721 | 1.000 | 1.000 1.000 1.000 | 1.000 | 71 | 1 |
| Some(9) | 72/72 | 24+24+24 | 28.4 | 15169701 | 1.045 | 0.966 1.081 1.044 | 1.005 | 71 | 0 |
| Some(1) | 70/72 | 24+23+23 | 29.9 | 15668360 | 1.079 | 1.059 1.310 0.993 | 1.005 | 69 | 2 |
| Some(2) | 67/72 | 24+22+21 | 28.1 | 16051245 | 1.105 | 1.079 1.177 1.082 | 1.007 | 66 | 6 |
| Some(3) | 68/72 | 24+23+21 | 29.3 | 16615959 | 1.144 | 1.074 1.244 1.117 | 1.004 | 67 | 7 |
| None, cap 200 | 71/72 | 23+24+24 | 29.9 | 14605393 | 1.006 | 1.056 1.000 1.000 | 1.000 | 71 | 1 |
| Some(9), cap 200 | 72/72 | 24+24+24 | 28.4 | 15169701 | 1.045 | 0.966 1.081 1.044 | 1.005 | 71 | 0 |

### Option sweep on the other families

Each cell is the number solved, the expansions as a multiple of the default row, and the cost as
a multiple of the default row over instances that both solved. A dash means that there were no
such instances. The sweep prints absolute figures, which are left out here for space.

#### `max_iterations`

| Value | Dense, other seeds | Multi-terminal grid | Island fabric | FPGA-like fabric |
| --- | --- | --- | --- | --- |
| 25 | 40/72, 0.932, 1.000 | 47/72, 0.878, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 50 | 68/72, 0.986, 1.000 | 61/72, 0.955, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 100 (default) | 71/72, 1.000, 1.000 | 67/72, 1.000, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 200 | 72/72, 1.004, 1.000 | 72/72, 1.013, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 400 | 72/72, 1.004, 1.000 | 72/72, 1.013, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |

#### `first_present`

| Value | Dense, other seeds | Multi-terminal grid | Island fabric | FPGA-like fabric |
| --- | --- | --- | --- | --- |
| 0.0 | 49/72, 2.261, 1.034 | 67/72, 1.416, 1.023 | 18/18, 2.498, 1.026 | 12/12, 1.076, 1.012 |
| 0.1 | 59/72, 1.191, 1.003 | 67/72, 1.239, 1.005 | 18/18, 1.336, 0.996 | 12/12, 1.052, 1.016 |
| 0.25 | 69/72, 1.071, 0.998 | 71/72, 0.978, 1.001 | 18/18, 1.187, 0.999 | 12/12, 1.025, 1.007 |
| 0.5 (default) | 71/72, 1.000, 1.000 | 67/72, 1.000, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 1.0 | 68/72, 0.999, 0.999 | 71/72, 0.919, 0.996 | 18/18, 0.860, 1.001 | 12/12, 1.031, 1.007 |
| 2.0 | 68/72, 1.082, 1.001 | 70/72, 0.870, 0.991 | 18/18, 0.794, 1.004 | 12/12, 1.032, 1.014 |

#### `initial_present`

| Value | Dense, other seeds | Multi-terminal grid | Island fabric | FPGA-like fabric |
| --- | --- | --- | --- | --- |
| 0.1 | 52/72, 1.745, 1.014 | 69/72, 1.594, 1.010 | 18/18, 1.300, 0.996 | 12/12, 1.521, 0.986 |
| 0.25 | 62/72, 1.203, 1.002 | 68/72, 1.219, 1.003 | 18/18, 1.122, 0.999 | 12/12, 1.198, 0.992 |
| 0.5 (default) | 71/72, 1.000, 1.000 | 67/72, 1.000, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 1.0 | 70/72, 1.023, 1.006 | 70/72, 0.868, 0.996 | 18/18, 0.925, 1.001 | 12/12, 0.860, 1.016 |
| 2.0 | 66/72, 1.141, 1.009 | 72/72, 0.763, 1.002 | 18/18, 0.900, 1.004 | 12/12, 0.826, 1.056 |

#### `present_growth`

| Value | Dense, other seeds | Multi-terminal grid | Island fabric | FPGA-like fabric |
| --- | --- | --- | --- | --- |
| 1.1 | 54/72, 2.834, 0.997 | 71/72, 3.029, 0.965 | 18/18, 1.224, 1.000 | 12/12, 2.029, 0.936 |
| 1.3 | 64/72, 1.358, 0.995 | 71/72, 1.311, 0.974 | 18/18, 1.057, 0.999 | 12/12, 1.334, 0.956 |
| 1.5 | 64/72, 1.177, 0.998 | 70/72, 1.074, 0.983 | 18/18, 1.006, 0.999 | 12/12, 1.136, 0.979 |
| 1.8 (default) | 71/72, 1.000, 1.000 | 67/72, 1.000, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 2.2 | 71/72, 1.052, 1.005 | 71/72, 0.930, 1.004 | 18/18, 0.951, 0.999 | 12/12, 0.935, 1.024 |
| 3.0 | 66/72, 1.016, 1.007 | 70/72, 0.893, 1.016 | 18/18, 0.947, 0.999 | 12/12, 0.979, 1.094 |
| 1.8, first_present 0 | 49/72, 2.261, 1.034 | 67/72, 1.416, 1.023 | 18/18, 2.498, 1.026 | 12/12, 1.076, 1.012 |
| 1.3, first_present 0 | 50/72, 2.854, 1.016 | 69/72, 1.897, 0.998 | 18/18, 2.881, 1.025 | 12/12, 1.383, 0.957 |

#### `history_factor`

| Value | Dense, other seeds | Multi-terminal grid | Island fabric | FPGA-like fabric |
| --- | --- | --- | --- | --- |
| 0.0 | 0/72, 6.234, - | 1/72, 4.760, 1.059 | 17/18, 1.164, 1.002 | 0/12, 3.029, - |
| 0.2 | 55/72, 1.540, 0.990 | 61/72, 0.940, 0.985 | 18/18, 0.997, 0.999 | 12/12, 1.082, 1.014 |
| 0.5 | 65/72, 1.090, 0.993 | 68/72, 0.814, 0.980 | 18/18, 0.993, 0.999 | 12/12, 0.990, 0.989 |
| 1.0 (default) | 71/72, 1.000, 1.000 | 67/72, 1.000, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| 2.0 | 63/72, 1.377, 1.023 | 70/72, 1.546, 1.043 | 18/18, 1.033, 1.002 | 12/12, 1.157, 1.041 |
| 5.0 | 43/72, 2.898, 1.058 | 67/72, 3.134, 1.126 | 17/18, 2.545, 1.005 | 6/12, 6.514, 1.210 |

#### `reroute_all`

| Value | Dense, other seeds | Multi-terminal grid | Island fabric | FPGA-like fabric |
| --- | --- | --- | --- | --- |
| false (default) | 71/72, 1.000, 1.000 | 67/72, 1.000, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| true | 67/72, 1.913, 1.001 | 71/72, 1.578, 0.992 | 18/18, 2.213, 1.004 | 12/12, 1.783, 0.980 |
| false, cap 200 | 72/72, 1.004, 1.000 | 72/72, 1.013, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| true, cap 200 | 71/72, 2.150, 1.001 | 72/72, 1.588, 0.992 | 18/18, 2.213, 1.004 | 12/12, 1.783, 0.980 |

#### `shuffle_seed`

| Value | Dense, other seeds | Multi-terminal grid | Island fabric | FPGA-like fabric |
| --- | --- | --- | --- | --- |
| None (default) | 71/72, 1.000, 1.000 | 67/72, 1.000, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| Some(9) | 65/72, 1.169, 1.006 | 67/72, 1.238, 1.018 | 18/18, 1.076, 1.003 | 12/12, 1.211, 1.058 |
| Some(1) | 66/72, 1.238, 1.009 | 69/72, 1.198, 1.015 | 18/18, 1.054, 1.002 | 12/12, 1.081, 1.032 |
| Some(2) | 65/72, 1.244, 1.011 | 72/72, 1.148, 1.010 | 18/18, 1.029, 1.001 | 12/12, 1.133, 1.040 |
| Some(3) | 66/72, 1.284, 1.004 | 68/72, 1.203, 1.016 | 18/18, 1.022, 1.002 | 11/12, 1.454, 1.053 |
| None, cap 200 | 72/72, 1.004, 1.000 | 72/72, 1.013, 1.000 | 18/18, 1.000, 1.000 | 12/12, 1.000, 1.000 |
| Some(9), cap 200 | 67/72, 1.234, 1.007 | 72/72, 1.259, 1.017 | 18/18, 1.076, 1.003 | 12/12, 1.211, 1.058 |

#### What giving up costs

The tight island fabric, on which nothing was solved at any cap.

| Cap | Solved | Expansions | / default | Overuse left |
| --- | --- | --- | --- | --- |
| 25 | 0/6 | 3734747 | 0.281 | 332 |
| 50 | 0/6 | 7317801 | 0.551 | 278 |
| 100 (default) | 0/6 | 13289981 | 1.000 | 210 |
| 200 | 0/6 | 24033692 | 1.808 | 161 |
| 400 | 0/6 | 43312148 | 3.259 | 140 |

### Refining trees after negotiation

The engine grows each net's tree incrementally, and has no option to build it any other way, so
Steiner construction inside negotiation has not been measured. This measures what the public
API allows: taking the final routes of a solved run with default options, and passing them
through `steiner::refine_tree` with default options. `validate::check`, which shares no search
code with the engine, gives the costs and the verdicts, and it also accepted all of the engine's
routes before refinement.

- **Blind** refines each net in the plain space, as if it were alone.
- **Residual** refines each net in turn, in a wrapper of the space that hides steps into
  resources which the other nets fill at that moment. One pass, in net order.
- **Alone** is what each net would cost with nobody else there, by
  `takahashi_matsuyama` or `kmb_tree`. It is a reference and not a legal result.

Costs are multiples of the negotiated cost. "Gap recovered" is how much of the distance from the
negotiated cost down to "alone, growth" the residual pass made up.

| Family | Solved | Nets | Negotiated cost | Alone, growth | Alone, KMB | Blind | Blind legal | Overused per instance, blind | Residual | Residual legal | Nets improved | Gap recovered | Expansions / engine, blind | Expansions / engine, residual |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| dense grids, tuning seeds | 71/72 | 2280 | 107926.0 | 0.922 | 0.922 | 0.922 | 0/71 | 389.2 | 0.995 | 71/71 | 201/2280 | 0.059 | 0.013 | 0.019 |
| dense grids, other seeds | 71/72 | 2280 | 108200.0 | 0.923 | 0.923 | 0.923 | 0/71 | 399.5 | 0.995 | 71/71 | 201/2280 | 0.063 | 0.015 | 0.020 |
| multi-terminal grid | 67/72 | 2800 | 202979.0 | 0.807 | 0.799 | 0.799 | 0/67 | 656.3 | 0.989 | 67/67 | 885/2800 | 0.055 | 0.025 | 0.065 |
| island fabric | 18/18 | 1440 | 29255.0 | 0.982 | 0.976 | 0.975 | 0/18 | 136.3 | 0.994 | 18/18 | 135/1440 | 0.353 | 1.588 | 0.584 |
| FPGA-like fabric | 12/12 | 4036 | 64253.8 | 0.758 | 0.750 | 0.746 | 0/12 | 426.8 | 0.978 | 12/12 | 502/4036 | 0.092 | 0.460 | 0.203 |

What this shows:

- **Refining blindly after negotiation is never safe.** It looks like a saving of 2.5% to 25%, but
  not one of the 239 results stayed free of overuse, with between about 140 and 660 resources
  overused per instance. Most of what it "recovers" is the detours that negotiation added
  to make the result legal.
- **Refining around the other nets is always legal, and recovers little**: 0.5% to 2.2%, which
  is 5.5% to 35% of the gap. It is cheap on the grids, at 2% to 7% of the engine's expansions, and
  not on the fabrics, at 20% and 58%. A single pass in net order is the simplest form of this,
  and is not a claim about the most that can be recovered.
- **Congestion costs more than the choice of tree here.** The gap from negotiation to "alone" is
  2% to 24%, against 0% to 1% between KMB and growth with nobody else there. The closest is the
  island fabric, at 1.8% against 0.6%. The dense grids have only two-terminal nets, on which the
  two are the same.

That is consistent with the recommendation to keep incremental growth inside negotiation, but it
does not test it. Using a better tree in each iteration could change how the negotiation goes,
and only an option in the engine could measure that.

### Timings

**These times come from one shared machine** with 192 hardware threads, on which other work was
running: its load average was 20 to 35 during these runs, and an earlier harness saw the same
run differ by up to 1.8 times between rounds. Treat them as rough. Prefer the counts above
wherever a count can answer the question, and compare times only between runs close together.

The default bench took 44 seconds in all. Each run is the whole of a `route` call with default
options, and the time is criterion's point estimate from 10 samples. `grid512/light`
stops at a cap of 4 iterations, with conflicts left, as it is only there to time expansions.

| Benchmark | Nets | Solved | Iterations | Expansions | Total cost | Time per run | ns per expansion |
| --- | --- | --- | --- | --- | --- | --- | --- |
| engine_dense/24x24x2 | 24 | no | 100 | 124423 | 834 | 18.142 ms | 146 |
| engine_dense/32x32x2 | 32 | yes | 12 | 145916 | 1426 | 20.857 ms | 143 |
| engine_dense/40x40x2 | 40 | yes | 67 | 448499 | 2278 | 64.659 ms | 144 |
| engine_dense/64x64x2 | 64 | yes | 17 | 1307508 | 5926 | 194.49 ms | 149 |
| engine_multi_terminal/32x32x2/28_nets | 28 | yes | 8 | 39523 | 1097 | 7.8737 ms | 199 |
| engine_multi_terminal/48x48x2/42_nets | 42 | yes | 11 | 180558 | 2609 | 32.007 ms | 177 |
| engine_multi_terminal/64x64x2/56_nets | 56 | yes | 85 | 1544649 | 5276 | 283.47 ms | 184 |
| engine_fabric/island/16x16/5_tracks | 80 | yes | 12 | 66206 | 1799 | 11.021 ms | 166 |
| engine_fabric/fpga_like/16x16/up_to_200_nets | 140 | yes | 9 | 113951 | 2062.5625 | 11.073 ms | 97 |
| engine_fabric/fpga_like/32x32/up_to_800_nets | 545 | yes | 9 | 485592 | 8556.8125 | 48.107 ms | 99 |
| engine_expansion_cost/grid512/light | 40 | no | 4 | 5010140 | 18420 | 685.61 ms | 137 |

Time per expansion, with no other sweep running. These runs also stop at a low cap with conflicts
left, as they are there to time expansions as the resource table fills up. The digest covers all
routes, overuse and cost. All three digests and expansion counts match those of the harness
that these runs come from, which saw fastest times of 142 to 194 ns on the first run and 201 to
234 on the second over three rounds, and 270 to 296 on the third over five, pinned to one core.

| Run | Nets | Iterations | Overused | Expansions | Digest | Repeats | Fastest, ns | Median, ns |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| grid512/light | 40 | 4 | 174 | 5010140 | 093707250f21a86e | 12 | 144 | 146 |
| grid512/heavy | 1000 | 2 | 121456 | 117916096 | 0691545f90124ea7 | 3 | 166 | 176 |
| fabric128 | 4000 | 6 | 812 | 212427637 | 2790fd092d7af567 | 3 | 264 | 269 |

### Timing-driven routing

Measured on 2026-11-04, on the commit that adds this section, with rustc 1.97.1.

**These are synthetic, and not a comparison with VPR**, which came later: see [FPGA routing
against VPR](#fpga-routing-against-vpr). The fabric, its delays and its netlist are all made up
here, as `benches/engine.rs` describes.

- **The fabrics** are the FPGA-like family above, 16 x 16 and 32 x 32 tiles at 1 track, and a
  third group at 32 x 32 with 2 tracks, which has room to spare. 6 seeds each, the same seeds.
- **Delays** are fixed per step: 40 ps into a pin, and 70 ps for a switch onto a wire plus 60
  ps per tile on a single wire and 30 ps per tile on a long one, which spans 4 tiles. So long
  wires are faster per tile, as on real devices, and also cheaper, so cost and delay agree on
  which wires are good but not by how much. The delay bound is the tile distance times
  47.5 ps, the fastest a long wire covers a tile.
- **The netlist** puts a register on one tile in four, and on the others a cell of 200 ps from
  the tile's sink pin to its source pin. A pin that no net reaches launches, and one that
  reaches no net captures. The clock period is 0, so that the fastest circuit is wanted, and
  criticality is each connection's share of the longest path through it. Net arcs start at
  50 ps per tile of distance. Combinational loops come out of the random nets, and the timing
  graph breaks them, as `src/timing.rs` describes.
- **A picosecond costs 1/50**, so that a tile's worth of delay costs about a tile's worth of
  wire. Criticality options are VPR's defaults.
- **"Alone"** is the critical path if every connection had its own fastest route, as if no
  other connection or net were there. No route can beat it for any one sink, so it is a floor.

Sums over each group's 6 instances. Ratios are to the first row, over instances both solved,
which here is all of them. Wirelength counts the steps of all routes. Every critical path here
was also worked out afresh from the routes, by the bench and not the engine, and matched what
the engine reported.

#### 16 x 16, 1 track, up to 200 nets

| Setting | Solved | Critical path, ps | / reference | / alone | Wirelength | / reference | Cost / reference | Iterations | Expansions | / reference |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| No timing (reference) | 6/6 | 70530 | 1.000 | 1.229 | 10228 | 1.000 | 1.000 | 52 | 622345 | 1.000 |
| Timing | 6/6 | 60000 | 0.851 | 1.046 | 10617 | 1.038 | 1.073 | 87 | 921050 | 1.480 |
| No timing, first_present 0 | 6/6 | 79550 | 1.128 | 1.387 | 10352 | 1.012 | 1.027 | 56 | 693533 | 1.114 |
| Timing, first_present 0 | 6/6 | 59960 | 0.850 | 1.045 | 10583 | 1.035 | 1.064 | 95 | 934261 | 1.501 |
| No timing, batch 32 | 6/6 | 76060 | 1.078 | 1.326 | 10476 | 1.024 | 1.046 | 70 | 1153927 | 1.854 |
| Timing, batch 32 | 6/6 | 59860 | 0.849 | 1.043 | 10689 | 1.045 | 1.087 | 91 | 1404907 | 2.257 |

Alone: 57370 ps.

#### 32 x 32, 1 track, up to 800 nets

| Setting | Solved | Critical path, ps | / reference | / alone | Wirelength | / reference | Cost / reference | Iterations | Expansions | / reference |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| No timing (reference) | 6/6 | 91270 | 1.000 | 1.337 | 42445 | 1.000 | 1.000 | 61 | 3081090 | 1.000 |
| Timing | 6/6 | 73070 | 0.801 | 1.070 | 43845 | 1.033 | 1.052 | 74 | 3956505 | 1.284 |
| No timing, first_present 0 | 6/6 | 88780 | 0.973 | 1.301 | 42712 | 1.006 | 1.008 | 65 | 3292157 | 1.069 |
| Timing, first_present 0 | 6/6 | 70560 | 0.773 | 1.034 | 43790 | 1.032 | 1.054 | 83 | 4107080 | 1.333 |
| No timing, batch 32 | 6/6 | 93910 | 1.029 | 1.376 | 42892 | 1.011 | 1.019 | 58 | 4117569 | 1.336 |
| Timing, batch 32 | 6/6 | 69850 | 0.765 | 1.023 | 44312 | 1.044 | 1.078 | 98 | 5349372 | 1.736 |

Alone: 68260 ps.

#### 32 x 32, 2 tracks, up to 800 nets

| Setting | Solved | Critical path, ps | / reference | / alone | Wirelength | / reference | Cost / reference | Iterations | Expansions | / reference |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| No timing (reference) | 6/6 | 70790 | 1.000 | 1.095 | 38357 | 1.000 | 1.000 | 30 | 923159 | 1.000 |
| Timing | 6/6 | 64670 | 0.914 | 1.000 | 38824 | 1.012 | 1.032 | 35 | 1201355 | 1.301 |
| No timing, first_present 0 | 6/6 | 69590 | 0.983 | 1.077 | 38519 | 1.004 | 1.016 | 30 | 1091078 | 1.182 |
| Timing, first_present 0 | 6/6 | 64670 | 0.914 | 1.000 | 38867 | 1.013 | 1.037 | 34 | 1345878 | 1.458 |
| No timing, batch 32 | 6/6 | 71110 | 1.005 | 1.100 | 38350 | 1.000 | 1.001 | 30 | 1083899 | 1.174 |
| Timing, batch 32 | 6/6 | 64670 | 0.914 | 1.000 | 38817 | 1.012 | 1.032 | 35 | 1349808 | 1.462 |

Alone: 64640 ps.

What this shows:

- **Timing shortens the critical path by 9% to 20%**, and brings it to within 0% to 7% of the
  floor, against 9% to 34% above it without timing. The price is 1% to 4% more wire, 3% to 7%
  more cost, and 28% to 48% more expansions. On the fabric with room to spare, timing reaches
  the floor to within 30 ps of 64640.
- **`first_present` with timing on**, which the docs of `EngineOptions` asked to have measured
  again: 0 gave the same critical path on two groups and one 3.4% shorter on the third, for 1.4%
  to 12% more expansions and more iterations. Without timing, 0 gave a critical path 12.8%
  longer on the small fabric, and 2.7% and 1.7% shorter on the others. Neither is a case for changing
  the default, and 18 synthetic instances are not enough to make one.
- **Batches of 32 did not hurt timing** here: the critical path was 0% to 4.4% shorter than
  timing-driven at a batch size of 1, for more expansions, as without timing.

**Time.** In the default bench, timing-driven runs of the FPGA-like fabric took 14.5 ms against
11.4 ms at 16 x 16, and 79 ms against 51 ms at 32 x 32, pinned to one core, over three rounds
each. Per expansion, that is 18% to 21% more, for reading a delay with each step and
weighing it, and the rest is the extra expansions. The timing analysis after each iteration
is a small part of it.

**What timing costs when it is not used.** The engine's results without timing are the same to
the bit, which every golden value and determinism test in the crate checks. For time, the
default bench's dense grid of 32, multi-terminal grid of 48, island fabric and both FPGA-like
fabrics were run against the commit before timing, alternating, three rounds each, pinned to
one core of the same shared machine at a load of about 35. The medians of the branch were 0.1%
to 2.8% slower on four of them and 0.5% faster on the island fabric, and the same binary moved
by up to 3.5% between rounds. The 2.8%, on the dense grid, is within that, and could be noise or
a small real cost: this machine can't tell the two apart.

#### Going on after a legal result

Measured on 2026-11-05, on the commit that adds `EngineOptions::timing_iterations`, by the same
sweep, which now has five more rows. The rows above are unchanged by it, to the digit. "10 more
at 0.9" goes on for up to 10 iterations after the first legal result, rerouting the nets of
connections of criticality 0.9 or more. Ratios here are to the "Timing" row, and every instance
was solved in every row.

| Group | Setting | Critical path, ps | / timing | Wirelength | Iterations | Expansions | / timing |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 16 x 16, 1 track | Timing | 60000 | 1.000 | 10617 | 87 | 921050 | 1.000 |
| | 10 more at 0.9 | 59290 | 0.988 | 10561 | 147 | 1405646 | 1.526 |
| | 30 more at 0.9 | 58020 | 0.967 | 10508 | 251 | 2088888 | 2.268 |
| | 10 more at 0.8 | 59290 | 0.988 | 10557 | 147 | 1446548 | 1.571 |
| | 10 more at 0.95 | 60000 | 1.000 | 10584 | 147 | 1395280 | 1.515 |
| | 10 more at 0.5 | 59290 | 0.988 | 10565 | 147 | 1797432 | 1.951 |
| 32 x 32, 1 track | Timing | 73070 | 1.000 | 43845 | 74 | 3956505 | 1.000 |
| | 10 more at 0.9 | 69780 | 0.955 | 43685 | 134 | 4998369 | 1.263 |
| | 30 more at 0.9 | 68740 | 0.941 | 43554 | 254 | 6469531 | 1.635 |
| | 10 more at 0.8 | 69780 | 0.955 | 43560 | 134 | 5616312 | 1.419 |
| | 10 more at 0.95 | 69780 | 0.955 | 43679 | 134 | 4745832 | 1.199 |
| | 10 more at 0.5 | 69780 | 0.955 | 43429 | 134 | 7623642 | 1.927 |
| 32 x 32, 2 tracks | Timing | 64670 | 1.000 | 38824 | 35 | 1201355 | 1.000 |
| | 10 more at 0.9 | 64670 | 1.000 | 38822 | 78 | 1437571 | 1.197 |
| | 30 more at 0.9 | 64670 | 1.000 | 38822 | 130 | 1709584 | 1.423 |
| | 10 more at 0.8 | 64670 | 1.000 | 38829 | 92 | 1818086 | 1.513 |
| | 10 more at 0.95 | 64670 | 1.000 | 38822 | 78 | 1415197 | 1.178 |
| | 10 more at 0.5 | 64670 | 1.000 | 38852 | 95 | 3217096 | 2.678 |

What this shows:

- **It helps few instances, and those a lot.** Per instance, 10 more iterations shortened 2 of
  the 18 critical paths, by 7.1% and 23%, and 30 shortened 5, by 3.2% to 23%. The other 13
  were unchanged, and none can get longer, as the first legal result stays a candidate. Over
  a group, that is 1.2% to 4.5% at 10 and 3.3% to 5.9% at 30, on the two narrow groups. The
  group with room to spare was already at the floor, so it had nothing to gain.
- **It costs 1.2 to 2.3 times the expansions** of plain timing-driven routing at a criticality
  of 0.9. The iterations nearly all run, because rerouting keeps changing routes.
- **The criticality matters little.** 0.5, 0.8 and 0.9 found the same critical paths, 0.95
  missed the gain on the small group, and lower values reroute more nets for more work.
- **Without restarting the present factor**, which was tried first, critical nets were
  rerouted against the prices that the negotiation had left and found the routes they had. Only
  one instance changed, from 14190 to 10900 ps, which is the 32 x 32 group's 4.5%, for 0.8% more
  expansions there, and the small group gained nothing. Runs then stopped as soon as an
  iteration changed nothing, which the engine still does.

So the default stays at 0: this is for a caller who wants the last few percent of a critical
path and can pay for it. On these instances, 10 iterations at 0.9 is a reasonable setting.

## What this does not show

- Nothing here is a real design. Dense permutation problems are a stress test, with terminals
  only on one layer and none of a board's or a device's structure. The fabrics are random
  graphs with a realistic shape, and small.
- The engine is sequential here, and these instances are far smaller than a large device's graph.
- 72 instances per family, at a few sizes, is few. See above for how much a count moves by chance.
- The defaults were tuned on the tuning seeds, and the other seeds only have the same shape.
- reticle reports searches flooding on a real FPGA without a charge in the first iteration, which
  did not happen on these instances, so the `first_present` results may not carry over at all.
- The delays of the timing sweep are fixed per step and made up, and its netlist is random.
  Real devices have Elmore delays that depend on the tree, and real netlists have structure.
  None of it is evidence of how timing-driven routing here compares with VPR's.

## FPGA and PCB comparisons

Of the comparisons of results in the [roadmap](../ROADMAP.md#measuring-best), the ones
done so far are below. There are no results for the others yet.

| Domain | Compared against | Benchmarks | Status |
| --- | --- | --- | --- |
| FPGA routing | VPR 9.0.0 | VTR's MCNC circuits | [Done](#fpga-routing-against-vpr) |
| FPGA routing | VPR 9.0.0 | VTR's Verilog suite (`vtr_reg_qor_chain`) | [Done](#vtrs-verilog-benchmarks) |
| FPGA routing | VPR, nextpnr | Titan23 | Not yet done |
| FPGA placement | VPR 9.0.0 | VTR's MCNC circuits and Verilog suite | [Done](#fpga-placement-against-vpr) |
| FPGA placement | nextpnr | VTR benchmarks, ISPD 2016/2017 contest sets | Not yet done |
| PCB clearance checks | KiCad 10.0.3 | [Curated open-source KiCad boards](../testdata/kicad/README.md) | [Done](#pcb-clearance-checks-against-kicad) |
| PCB DSN import, SES export | KiCad 10.0.3's reading of the same boards, and its session import | The same boards | [Done](#pcb-dsn-designs-against-kicad-boards) |
| PCB routing | Freerouting 2.5.0, both checked by KiCad 10.0.3 | The same boards | [First results](#pcb-routing-against-freerouting) |
| PCB placement | Hand-placed originals | The same boards | Not yet done |
| ASIC global routing | Published contest results | ISPD 2007/2008 | Not yet done |

## FPGA routing against VPR

This is the comparison that M3's exit criterion asks for: `guide`'s router against VPR's, on
the same placements and VPR's own device graphs, with VPR checking and timing every route of
`guide`'s.

Measured on 2026-11-10 with rustc 1.97.1, on an AMD Ryzen Threadripper PRO 9995WX shared
with other work. "Before" is `14c1022`, with the [lookahead and incremental
rerouting](#closing-the-runtime-gap) and constant nets left unrouted as VPR leaves them. The
rest is `7346331`, whose `RrGraph::engine_options` were [tuned on both
suites](#tuning-the-negotiation-on-both-suites). VPR, the circuits and the placements are the
same in both, and so are VPR's results, but for its times. The first results of this section,
from `e2d0bd5`, took 3.2 times VPR's time: [closing the runtime gap](#closing-the-runtime-gap)
has how that went.

### How

- **VPR** is VPR 9.0.0, VTR's tag `v9.0.0`, commit `8cb20aa5293b5ce5f6a05c88b37b38e1305d9b07`,
  built by [`scripts/vtr/build-vpr.sh`](../scripts/vtr/build-vpr.sh) with nothing but VPR.
- **The circuits** are the 20 largest MCNC circuits, the set that VPR's papers have used since
  VPR's first, as VTR's `vpr_reg_mcnc_equiv` regression test has them: technology-mapped to
  6-input LUTs (`vtr_flow/benchmarks/blif/wiremap6/*.pre-vpr.blif`), on
  `vtr_flow/arch/timing/k6_N10_40nm.xml`, clusters of ten 6-LUTs with unidirectional
  wires of length 4. They are not copied here, as VTR's licence excepts its benchmark
  circuits. VTR's own Verilog suite, with memories and multipliers, is
  [below](#vtrs-verilog-benchmarks). Titan23 was not run.
- **The placement** is VPR's, packed and placed once per circuit with seed 1, and both
  routers start from it.
- **Minimum channel width.** VPR finds its own by its binary search. `guide` finds its own
  with the same rules: even widths only, as the wires are unidirectional, at most 50
  iterations at each, as VPR allows, giving up early as VPR's routing failure predictor
  does (`EngineOptions::give_up_factor`), and a binary search between a width that fails and
  one that routes, starting from VPR's. Each width's graph is the one VPR writes for it with
  `--write_rr_graph`.
- **Delay, wirelength and runtime** are at 1.3 times VPR's minimum width, rounded to an even
  number as VTR's flow does (`relax_w` in its scripts), on the graph that VPR wrote while
  routing at that width itself. Delays of both are VPR's timing analysis: each route of
  `guide`'s is loaded back with `vpr --analysis --route_file`, which checks it as it checks its
  own (`check_route`, including its reserved output pins) and then times it. Wirelength is
  VPR's count of tiles of wire, which agrees with `guide`'s own count in every case.
- **Runtime** is the time inside the router, on one thread for both: VPR's `# Routing took`
  line with `-j 1`, which leaves out building the graph and VPR's lookahead, and `guide`'s
  route time in `examples/vtr_route.rs`, which leaves out reading the files and its own
  lookahead but includes building the timing graph. Both lookaheads are shown separately.
  "Threads" is `guide` with 16 threads and batches of 32, which gives different routes from
  batches of 1, so it has its own delay.
- **`guide`'s options** are `RrGraph::engine_options()` after `RrGraph::use_map_lookahead`
  with its defaults, timing-driven with `DesignTiming::vpr_constraints(0)`, VPR's default
  without an SDC file. Before, they had a present factor growth of 1.8, a history of one
  tile of wire, and did not give up early.

```text
scripts/vtr/build-vpr.sh DIR                     # about two minutes on many cores
JOBS=8 scripts/vtr/compare.sh DIR WORK           # all 20 circuits, a few minutes on many cores
python3 scripts/vtr/tables.py WORK [BEFORE]      # the tables below, BEFORE for a column
                                                 # of an earlier run's
```

### Results

**Every route is legal**, by the validator and by VPR itself: all 20 circuits at `guide`'s
minimum width and at the relaxed width, single-threaded and threaded, 60 routes in all. Ratios
are `guide`'s over VPR's, so below 1 is better for `guide`, and the last row is their geometric
mean.

Minimum channel width:

| Circuit | CLBs | Grid, tiles a side | VPR | guide before | before / VPR | guide | guide / VPR | VPR accepts guide's route |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 106 | 13 | 40 | 38 | 0.950 | 36 | 0.900 | yes |
| apex2 | 126 | 14 | 54 | 52 | 0.963 | 52 | 0.963 | yes |
| apex4 | 105 | 13 | 56 | 54 | 0.964 | 52 | 0.929 | yes |
| bigkey | 92 | 16 | 34 | 34 | 1.000 | 34 | 1.000 | yes |
| clma | 435 | 23 | 72 | 66 | 0.917 | 68 | 0.944 | yes |
| des | 102 | 18 | 34 | 34 | 1.000 | 34 | 1.000 | yes |
| diffeq | 102 | 13 | 30 | 30 | 1.000 | 28 | 0.933 | yes |
| dsip | 93 | 16 | 34 | 34 | 1.000 | 36 | 1.059 | yes |
| elliptic | 242 | 18 | 52 | 48 | 0.923 | 46 | 0.885 | yes |
| ex1010 | 322 | 20 | 86 | 82 | 0.953 | 76 | 0.884 | yes |
| ex5p | 98 | 12 | 44 | 44 | 1.000 | 44 | 1.000 | yes |
| frisc | 251 | 18 | 58 | 54 | 0.931 | 54 | 0.931 | yes |
| misex3 | 100 | 12 | 46 | 42 | 0.913 | 42 | 0.913 | yes |
| pdc | 332 | 21 | 72 | 64 | 0.889 | 64 | 0.889 | yes |
| s298 | 84 | 12 | 40 | 40 | 1.000 | 34 | 0.850 | yes |
| s38417 | 397 | 22 | 34 | 32 | 0.941 | 32 | 0.941 | yes |
| s38584.1 | 360 | 21 | 40 | 38 | 0.950 | 36 | 0.900 | yes |
| seq | 112 | 13 | 54 | 52 | 0.963 | 54 | 1.000 | yes |
| spla | 265 | 19 | 60 | 56 | 0.933 | 54 | 0.900 | yes |
| tseng | 112 | 13 | 26 | 26 | 1.000 | 26 | 1.000 | yes |
| Geomean |  |  |  |  | 0.959 |  | 0.940 |  |

At the relaxed width, the critical-path delay, by VPR's analysis of each tool's routes, and
by `guide`'s own analysis of its routes:

| Circuit | W | VPR, ns | guide before, ns | before / VPR | guide, ns | guide / VPR | guide's own STA, ns | own / VPR's STA | VPR accepts guide's route |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 52 | 5.318 | 4.807 | 0.904 | 4.846 | 0.911 | 4.842 | 0.999 | yes |
| apex2 | 70 | 5.807 | 5.777 | 0.995 | 5.777 | 0.995 | 5.772 | 0.999 | yes |
| apex4 | 72 | 5.386 | 5.281 | 0.980 | 5.278 | 0.980 | 5.272 | 0.999 | yes |
| bigkey | 44 | 3.307 | 3.264 | 0.987 | 3.265 | 0.987 | 3.304 | 1.012 | yes |
| clma | 94 | 8.695 | 8.662 | 0.996 | 8.665 | 0.997 | 8.654 | 0.999 | yes |
| des | 44 | 4.469 | 4.460 | 0.998 | 4.464 | 0.999 | 4.459 | 0.999 | yes |
| diffeq | 40 | 6.095 | 6.199 | 1.017 | 6.103 | 1.001 | 6.098 | 0.999 | yes |
| dsip | 44 | 3.179 | 3.176 | 0.999 | 3.176 | 0.999 | 3.214 | 1.012 | yes |
| elliptic | 68 | 7.581 | 7.548 | 0.996 | 7.546 | 0.995 | 7.538 | 0.999 | yes |
| ex1010 | 112 | 6.897 | 6.823 | 0.989 | 6.821 | 0.989 | 6.813 | 0.999 | yes |
| ex5p | 58 | 4.621 | 4.556 | 0.986 | 4.549 | 0.984 | 4.544 | 0.999 | yes |
| frisc | 76 | 10.007 | 9.812 | 0.980 | 9.592 | 0.959 | 9.585 | 0.999 | yes |
| misex3 | 60 | 4.821 | 4.786 | 0.993 | 4.784 | 0.992 | 4.779 | 0.999 | yes |
| pdc | 94 | 7.060 | 7.052 | 0.999 | 7.052 | 0.999 | 7.042 | 0.999 | yes |
| s298 | 52 | 7.263 | 7.494 | 1.032 | 7.352 | 1.012 | 7.345 | 0.999 | yes |
| s38417 | 44 | 5.800 | 5.512 | 0.950 | 5.512 | 0.950 | 5.506 | 0.999 | yes |
| s38584.1 | 52 | 5.056 | 4.879 | 0.965 | 4.880 | 0.965 | 4.841 | 0.992 | yes |
| seq | 70 | 4.872 | 4.823 | 0.990 | 4.794 | 0.984 | 4.789 | 0.999 | yes |
| spla | 78 | 6.529 | 6.482 | 0.993 | 6.517 | 0.998 | 6.508 | 0.999 | yes |
| tseng | 34 | 5.713 | 5.684 | 0.995 | 5.684 | 0.995 | 5.679 | 0.999 | yes |
| Geomean |  |  |  | 0.987 |  | 0.984 |  | 1.000 |  |

Wirelength at the relaxed width, in tiles of wire:

| Circuit | VPR | guide before | before / VPR | guide | guide / VPR |
| --- | --- | --- | --- | --- | --- |
| alu4 | 7291 | 6840 | 0.938 | 6951 | 0.953 |
| apex2 | 12151 | 12015 | 0.989 | 11739 | 0.966 |
| apex4 | 10212 | 9338 | 0.914 | 9352 | 0.916 |
| bigkey | 6415 | 6328 | 0.986 | 6263 | 0.976 |
| clma | 44365 | 42623 | 0.961 | 42652 | 0.961 |
| des | 6744 | 6580 | 0.976 | 6619 | 0.981 |
| diffeq | 5218 | 4749 | 0.910 | 4809 | 0.922 |
| dsip | 7198 | 6918 | 0.961 | 6917 | 0.961 |
| elliptic | 16865 | 15349 | 0.910 | 15326 | 0.909 |
| ex1010 | 40817 | 38710 | 0.948 | 38845 | 0.952 |
| ex5p | 6964 | 6771 | 0.972 | 6671 | 0.958 |
| frisc | 21617 | 19599 | 0.907 | 19669 | 0.910 |
| misex3 | 6920 | 6791 | 0.981 | 6861 | 0.991 |
| pdc | 36688 | 34460 | 0.939 | 34864 | 0.950 |
| s298 | 5886 | 5210 | 0.885 | 5232 | 0.889 |
| s38417 | 18574 | 17595 | 0.947 | 17667 | 0.951 |
| s38584.1 | 19108 | 18080 | 0.946 | 18154 | 0.950 |
| seq | 10428 | 9654 | 0.926 | 9903 | 0.950 |
| spla | 25425 | 23896 | 0.940 | 24047 | 0.946 |
| tseng | 4210 | 4077 | 0.968 | 4050 | 0.962 |
| Geomean |  |  | 0.945 |  | 0.947 |

Runtime at the relaxed width. Each ratio is to VPR's time in the same run, as VPR's times move
from run to run:

| Circuit | VPR, s | VPR's lookahead, s | VPR iterations | guide before, s | before / VPR | guide, 1 thread, s | guide's lookahead, s | guide iterations | / VPR | guide, threads, s | / VPR | Its delay, ns | VPR accepts it |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 0.13 | 0.01 | 24 | 0.10 | 0.37 | 0.05 | 0.02 | 11 | 0.39 | 0.10 | 0.73 | 4.846 | yes |
| apex2 | 0.21 | 0.02 | 24 | 0.07 | 0.33 | 0.07 | 0.03 | 10 | 0.33 | 0.12 | 0.55 | 5.782 | yes |
| apex4 | 0.18 | 0.02 | 27 | 0.06 | 0.26 | 0.06 | 0.02 | 11 | 0.33 | 0.09 | 0.48 | 5.282 | yes |
| bigkey | 0.10 | 0.02 | 16 | 0.04 | 0.33 | 0.03 | 0.03 | 9 | 0.28 | 0.04 | 0.44 | 3.265 | yes |
| clma | 0.93 | 0.10 | 23 | 0.21 | 0.21 | 0.30 | 0.12 | 13 | 0.32 | 0.34 | 0.37 | 8.661 | yes |
| des | 0.08 | 0.02 | 14 | 0.02 | 0.26 | 0.02 | 0.04 | 7 | 0.21 | 0.04 | 0.55 | 4.464 | yes |
| diffeq | 0.09 | 0.01 | 22 | 0.04 | 0.18 | 0.02 | 0.02 | 9 | 0.19 | 0.04 | 0.41 | 6.200 | yes |
| dsip | 0.13 | 0.02 | 18 | 0.04 | 0.26 | 0.03 | 0.03 | 9 | 0.23 | 0.05 | 0.35 | 3.176 | yes |
| elliptic | 0.26 | 0.03 | 24 | 0.07 | 0.23 | 0.08 | 0.05 | 11 | 0.30 | 0.13 | 0.48 | 7.554 | yes |
| ex1010 | 0.80 | 0.08 | 23 | 0.27 | 0.54 | 0.34 | 0.10 | 11 | 0.43 | 0.45 | 0.57 | 6.819 | yes |
| ex5p | 0.10 | 0.01 | 19 | 0.09 | 0.41 | 0.04 | 0.02 | 11 | 0.38 | 0.07 | 0.67 | 4.569 | yes |
| frisc | 0.48 | 0.04 | 25 | 0.07 | 0.17 | 0.08 | 0.05 | 9 | 0.16 | 0.14 | 0.30 | 9.595 | yes |
| misex3 | 0.11 | 0.01 | 20 | 0.07 | 0.30 | 0.05 | 0.02 | 12 | 0.42 | 0.09 | 0.81 | 4.783 | yes |
| pdc | 0.72 | 0.06 | 22 | 0.21 | 0.27 | 0.28 | 0.10 | 12 | 0.38 | 0.42 | 0.59 | 7.052 | yes |
| s298 | 0.10 | 0.01 | 21 | 0.06 | 0.38 | 0.03 | 0.02 | 10 | 0.28 | 0.05 | 0.52 | 7.505 | yes |
| s38417 | 0.32 | 0.04 | 25 | 0.06 | 0.18 | 0.07 | 0.06 | 9 | 0.23 | 0.14 | 0.45 | 5.512 | yes |
| s38584.1 | 0.34 | 0.04 | 22 | 0.09 | 0.26 | 0.09 | 0.06 | 11 | 0.26 | 0.14 | 0.41 | 4.873 | yes |
| seq | 0.17 | 0.02 | 30 | 0.07 | 0.25 | 0.06 | 0.02 | 12 | 0.36 | 0.10 | 0.61 | 4.795 | yes |
| spla | 0.60 | 0.06 | 24 | 0.17 | 0.31 | 0.22 | 0.07 | 11 | 0.37 | 0.34 | 0.56 | 6.492 | yes |
| tseng | 0.07 | 0.01 | 17 | 0.02 | 0.24 | 0.02 | 0.01 | 8 | 0.24 | 0.04 | 0.50 | 5.685 | yes |
| Geomean |  |  |  |  | 0.28 |  |  |  | 0.30 |  | 0.50 |  |  |

What this shows:

- **Channel width: `guide` needs 6.0% fewer tracks than VPR**, where it needed 4.1% fewer
  before. It needs fewer on 14 circuits, by up to 15% (s298, 34 against 40), as many on 5, and
  more on one, dsip, 36 against 34. Against before, it needs fewer on 8 circuits, ex1010 76
  against 82 and s298 34 against 40, and more on 3: clma, dsip and seq, by one step of two
  tracks each.
- **Delay: 1.6% shorter critical paths than VPR**, where they were 1.3% shorter. 18 of 20 are
  as fast as VPR's or faster, diffeq is 0.1% slower and s298 1.2%, where they were 1.7% and
  3.2% slower before. Single circuits move by up to 4% either way between configurations,
  which is more than the means do.
- **Wirelength: 5.3% less wire than VPR**, where it was 5.5% less, which is within what
  circuits move by between runs of different options. Every circuit uses less than VPR's.
- **Runtime: `guide` takes 0.30 times VPR's time**, where it took 0.28, from 0.16 times on
  frisc to 0.43 on ex1010. It takes 10.2 iterations at the relaxed width where it took 9.0,
  against VPR's 21.7, and 5% more expansions. With each tool's lookahead counted in, it
  takes 0.42 times VPR's time, where it took 0.40.
- **Threads still did not help** here: 16 threads with batches of 32 took 1.7 times as long as
  one thread with batches of 1. They do since, at 0.84 times on this suite and 0.56 on the
  Verilog one: see [threads on the larger circuits](#threads-on-the-larger-circuits).
- **`guide`'s own timing analysis agrees with VPR's**, by the worst slack as VPR counts it,
  to within 0.1% on 17 circuits, 0.8% under it on s38584.1, and 1.2% over it, pessimistic,
  on bigkey and dsip.

Before, between `21d5205`, which finished [the lookahead and incremental
rerouting](#closing-the-runtime-gap), and `14c1022`, `guide` started leaving constant nets
unrouted as VPR does (`fpga::constant_nets`), for [the Verilog
benchmarks](#vtrs-verilog-benchmarks). Three of these circuits have any: apex4 one, clma 15
and s38584.1 two. That took apex4's minimum width from 52 to 54, clma's from 68 to 66 and
s38584.1's from 36 to 38, and the geomean from 0.956 to 0.959 of VPR's, with every other
number the same but for times.

### Closing the runtime gap

Before, VPR pushed about 40 entries on its heap per connection on ex1010, and `guide` took
about 290 expansions. Three things closed the gap, and the table below has each of them on
its own and together, over all 20 circuits as above.

1. **A map lookahead**, `RrGraph::use_map_lookahead`, after VPR's `map` lookahead. For each
   type of wire, by axis, direction and segment, a search of the whole graph starts from every
   track of that type driven at a place on the device, once for the least sum of base costs
   to each `SINK` and once for the least delay, and the least over the places goes in a table
   by type and offset. Two places per axis, near opposite corners, are the fewest that see
   every offset. The graph's estimator takes the larger of the table and the location bound
   it had before. A minimum over places is not a minimum over all wires, so this is not
   exactly a lower bound: on small irregular islands in the tests, about one pair in ten of a
   wire and a `SINK` comes out above its distance. The lookahead also knows that an `IPIN`
   reaches only its own `SINK` and that a `SINK` reaches nothing, which is exact, and comes
   out as `Cost::MAX`. Searches hold those nodes back until nothing else is left, as [the
   search's documentation](../src/search.rs) explains, which never changes a result. On
   ex1010 that halved the pushes and took about a fifth of the time off.
2. **Incremental rerouting**, `EngineOptions::incremental_reroute`, as VPR does it. Before, a
   net that held one overused node was ripped up and routed whole, so ex1010's net of 689
   sinks took 689 searches each time, and 87% of ex1010's searches came after the first
   iteration. Now a net keeps its tree up to each overused node, and only the sinks cut off
   are routed again. It also gave less wire and narrower channels, which routing whole nets
   again does not seem to have been buying.
3. **A weight of 1.1 on the bound**, `EngineOptions::bound_weight`, as VPR's `--astar_fac`,
   whose default is 1.2.

`RrGraph::engine_options` turns on the second, and the third once the first is in use. The
engine's defaults are unchanged, and so is every result on grids and other graphs.

| Run | Min W / VPR | Delay / VPR | Wire / VPR | Time / VPR | Expansions / before | Time / before |
| --- | --- | --- | --- | --- | --- | --- |
| Before | 0.974 | 0.984 | 0.971 | 3.23 | 1 | 1 |
| Map lookahead | 0.974 | 0.988 | 0.971 | 0.52 | 0.057 | 0.16 |
| Map, weight 1.1 | 0.971 | 0.988 | 0.972 | 0.47 | 0.047 | 0.14 |
| Map, weight 1.2 | 0.983 | 0.988 | 0.966 | 0.47 | 0.045 | 0.14 |
| Map, weight 1.4 | 0.977 | 0.991 | 0.972 | 0.47 | 0.044 | 0.15 |
| Location bound, incremental | 0.952 | 0.989 | 0.943 | 1.91 | 0.539 | 0.61 |
| Map, incremental | 0.961 | 0.987 | 0.946 | 0.29 | 0.030 | 0.12 |
| **Map, incremental, weight 1.1** | **0.956** | **0.987** | **0.945** | **0.28** | **0.025** | **0.08** |
| Map, incremental, weight 1.2 | 0.959 | 0.991 | 0.943 | 0.27 | 0.024 | 0.12 |

The row in bold is the run that these tables had at `21d5205`, before constant nets and [the
tuning](#tuning-the-negotiation-on-both-suites). The others ran three or four at a time, so
their times are less reliable than their expansions, which do not depend on the machine.

What did not help, or not enough to keep:

- **Weights above 1.1.** 1.2, VPR's default, took about 5% fewer expansions than 1.1 for
  critical paths 0.4% longer. Without incremental rerouting, 1.2 and 1.4 also needed wider
  channels than 1.1, and 1.4 gave longer critical paths.
- **More places to search from.** Three per axis rather than two, on ex1010, clma and tseng,
  gave nearly the same expansions (ex1010 876 000 against 900 000, clma about the same) for
  twice the time to build, and one, the middle of the device, gave 1.1 to 3.4 times the
  expansions, as offsets beyond half the device have no entry there.
- **Rerouting critical connections whole.** VPR also rips up connections that are critical,
  with `--incremental_reroute_delay_ripup`. Cutting the branches to terminals of criticality
  0.9 or more from what is kept made critical paths 0.1% shorter, and 0.7 or more 0.2%, for
  3% and 36% more expansions: less than circuits move on their own, so it was left out.
- **Not queueing what cannot reach the target, without changing anything else.** Leaving
  out the pushes of the pins of other blocks on ex1010, 13.7 million of them down to 8.2,
  saved nothing measurable: the heap was never the cost. Leaving out the tree's own pins and
  sinks as sources of each search, two thirds of them, saved about a quarter of the time,
  which is the part that holding them back in the search kept.
- **Searching from the part of the tree near a sink only**, as VPR does for nets of high
  fanout, was not built. Once nets are rerouted incrementally, starting each search from the
  whole tree is about a fifth of ex1010's time, 50 ms of its 0.25 s, and most of the rest is
  spent expanding nodes, at under a microsecond each. The engine knows nothing of where a node
  is, so it would need a way for a space to say which nodes are near, which is a change to
  `RoutingSpace`.
- **Profiles.** perf hangs at the end of every recording on this machine and valgrind cannot
  run the binaries, so the constant factors were found by timing parts of the router. One
  thing showed up: `f64::floor` in the delay bound is a call into libm on x86-64 without
  SSE4.1, and rounding down by `as` gives the same values.

### What it took

Before VPR saw them, `guide`'s routes looked fine to its own validator, and two things were
wrong.

- **VPR rejected every route** at first: `guide` let a net leave a logic block through two of
  its output pins, which VPR does not allow when they are equivalent by `instance`, as here.
  VPR reserves every output pin that carries no net out of the block, so 61 nets of tseng
  overused 65 pins. A driver whose `SOURCE` leads to several `OPIN`s now has the `OPIN`s as its
  access points, so that a route takes one of them: see `fpga::Design`.
- **History was ten orders of magnitude too strong.** `EngineOptions::history_factor` is added
  to step costs, and its default of 1 suits costs of about 1, while a VPR graph's costs are in
  seconds, about 1e-10 for a tile of wire. A node overused once became a wall for the rest of
  the run. With the engine's defaults, over the 18 circuits that finished (clma and ex1010
  were stopped after more than half an hour), `guide` needed 1.17 times VPR's channel width,
  gave critical paths 1.44 times as long and 1.32 times the wire, and took 21 times as long.
  `RrGraph::engine_options` sets the factor to one tile of wire, which gave the results above.
  Four tiles, a whole wire and closer to VPR's own history, came out slightly worse on all
  three counts (0.986, 0.988 and 0.994 of VPR's).

Two quirks of VPR 9.0.0 that the scripts work around: it rewrites the placement file on every
run, with the netlist's path in its header, so the digest that a route file has to name changes
(`print_place` in `vpr_api.cpp` is outside the `if` of the analytical placer), and with GCC 15
it only builds with some standard headers forced in.

### What this does not show

- The MCNC circuits are small by today's standards, the largest 435 clusters on a 23 by 23
  grid, and of LUTs and flip-flops only. VTR's own Verilog suite, [below](#vtrs-verilog-benchmarks),
  is larger and has memories, multipliers and carry chains. Titan23 is larger still.
- One placement per circuit, from one seed. VPR's results move with the placement, and so
  may the comparison.
- One architecture. VPR's defaults were tuned for architectures like it, and so, now, is
  `engine_options`.
- The machine was shared, and times of a few tenths of a second move by tens of percent between
  runs. The ratios of the larger circuits are steadier. The times are now so short that the
  lookahead, which VPR leaves out of its routing time and so do these tables, takes as long
  as `guide`'s routing or longer on the smallest circuits.
- The map lookahead was only measured on this one architecture, with one type of wire. A
  device with several wire types or very irregular edges may see more of its entries above
  the true cost.

### VTR's Verilog benchmarks

The same comparison on VTR's own benchmark suite, on its flagship architecture: the circuits
of VTR's `vtr_reg_qor_chain` test. They are larger than the MCNC circuits, up to LU32PEEng's
7600 clusters, 32 multipliers and 167 memories on a 103 by 103 grid, and have multipliers,
memories and carry chains.

Measured on 2026-11-10 with rustc 1.97.1, on the same shared machine. "Before" is
`2bc0139`, the commit that made these routes acceptable to VPR, as [what it
took](#what-the-verilog-benchmarks-took) says, and the rest is `7346331`, whose
`RrGraph::engine_options` were [tuned on both suites](#tuning-the-negotiation-on-both-suites).
Titan23 was not run: see [what this does not show](#what-the-verilog-benchmarks-do-not-show).

#### How

As [for MCNC](#how), with these differences:

- **The circuits** are the 21 of `vtr_reg_qor_chain` (in `vtr_reg_nightly_test3_odin`), from
  `vtr_flow/benchmarks/verilog`, synthesized by VTR's own flow, `run_vtr_flow.py` from Odin II
  to ABC, as that test does, with Odin II and ABC built from the same VTR commit. The
  synthesized circuits are not copied here either.
- **The architecture** is `vtr_flow/arch/timing/k6_frac_N10_frac_chain_mem32K_40nm.xml`:
  clusters of ten fracturable 6-LUTs with carry chains between clusters, 36-bit multipliers four
  tiles tall, 32 Kb memories six tiles tall in several modes, and unidirectional wires of length
  4, so widths are even here too.
- **`guide`'s own timing analysis** is given as its worst slack against a period of 0, which
  counts the setup time at the end of the path as VPR does, where the MCNC tables have the
  latest arrival. Delays compared with VPR's are VPR's analysis of both tools' routes, as for
  MCNC.

```text
SYNTH=1 scripts/vtr/build-vpr.sh DIR             # VPR, Odin II and ABC
scripts/vtr/synth.sh DIR BLIF                    # the 21 circuits, 40 minutes for mcml
SUITE=verilog BLIF=BLIF scripts/vtr/compare.sh DIR WORK  # 1.5 hours, mostly
python3 scripts/vtr/tables.py WORK                       # LU32PEEng's two searches
```

`synth.sh` runs VTR's flow scripts, which need Python's `prettytable`.

#### Results

**Every route is legal**, by the validator and by VPR itself: all 21 circuits at `guide`'s
minimum width and at the relaxed width, single-threaded and threaded, 63 routes in all, and
VPR's count of constant nets agrees with `guide`'s on every circuit. Ratios are `guide`'s over
VPR's, so below 1 is better for `guide`, and the last row is their geometric mean.

Minimum channel width, with the hard blocks each circuit uses:

| Circuit | CLBs | Multipliers | Memories | Grid, tiles a side | VPR | guide before | before / VPR | guide | guide / VPR | VPR accepts guide's route |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 7600 | 32 | 167 | 103 | 124 | 122 | 0.984 | 116 | 0.935 | yes |
| LU8PEEng | 2232 | 8 | 44 | 57 | 92 | 88 | 0.957 | 84 | 0.913 | yes |
| arm_core | 1023 | 0 | 24 | 39 | 104 | 102 | 0.981 | 100 | 0.962 | yes |
| bgm | 2786 | 11 | 0 | 63 | 70 | 70 | 1.000 | 70 | 1.000 | yes |
| blob_merge | 619 | 0 | 0 | 31 | 58 | 62 | 1.069 | 60 | 1.034 | yes |
| boundtop | 97 | 0 | 0 | 14 | 42 | 42 | 1.000 | 42 | 1.000 | yes |
| ch_intrinsics | 68 | 0 | 1 | 12 | 42 | 42 | 1.000 | 42 | 1.000 | yes |
| diffeq1 | 32 | 5 | 0 | 16 | 60 | 60 | 1.000 | 54 | 0.900 | yes |
| diffeq2 | 21 | 7 | 0 | 18 | 46 | 50 | 1.087 | 42 | 0.913 | yes |
| mcml | 7302 | 27 | 159 | 101 | 124 | 126 | 1.016 | 112 | 0.903 | yes |
| mkDelayWorker32B | 473 | 0 | 45 | 50 | 38 | 40 | 1.053 | 40 | 1.053 | yes |
| mkPktMerge | 31 | 0 | 15 | 28 | 46 | 46 | 1.000 | 46 | 1.000 | yes |
| mkSMAdapter4B | 189 | 0 | 5 | 20 | 50 | 50 | 1.000 | 50 | 1.000 | yes |
| or1200 | 258 | 1 | 2 | 27 | 92 | 92 | 1.000 | 76 | 0.826 | yes |
| raygentop | 123 | 8 | 0 | 19 | 58 | 56 | 0.966 | 54 | 0.931 | yes |
| sha | 155 | 0 | 0 | 17 | 68 | 72 | 1.059 | 56 | 0.824 | yes |
| spree | 64 | 1 | 3 | 14 | 62 | 62 | 1.000 | 62 | 1.000 | yes |
| stereovision0 | 725 | 0 | 0 | 34 | 48 | 50 | 1.042 | 48 | 1.000 | yes |
| stereovision1 | 770 | 40 | 0 | 40 | 80 | 82 | 1.025 | 78 | 0.975 | yes |
| stereovision2 | 1977 | 324 | 0 | 104 | 68 | 72 | 1.059 | 60 | 0.882 | yes |
| stereovision3 | 14 | 0 | 0 | 7 | 30 | 30 | 1.000 | 30 | 1.000 | yes |
| Geomean |  |  |  |  |  |  | 1.014 |  | 0.953 |  |

At the relaxed width, the critical-path delay, by VPR's analysis of each tool's routes, and
by `guide`'s own analysis of its routes:

| Circuit | W | VPR, ns | guide before, ns | before / VPR | guide, ns | guide / VPR | guide's own STA, ns | own / VPR's STA | VPR accepts guide's route |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 162 | 76.286 | 76.128 | 0.998 | 76.099 | 0.998 | 76.040 | 0.999 | yes |
| LU8PEEng | 120 | 76.110 | 75.649 | 0.994 | 75.675 | 0.994 | 75.622 | 0.999 | yes |
| arm_core | 136 | 18.622 | 18.602 | 0.999 | 18.598 | 0.999 | 18.592 | 1.000 | yes |
| bgm | 92 | 20.456 | 20.471 | 1.001 | 20.464 | 1.000 | 20.450 | 0.999 | yes |
| blob_merge | 76 | 15.232 | 15.162 | 0.995 | 15.161 | 0.995 | 15.153 | 0.999 | yes |
| boundtop | 54 | 3.555 | 3.552 | 0.999 | 3.553 | 0.999 | 3.550 | 0.999 | yes |
| ch_intrinsics | 54 | 2.496 | 2.520 | 1.010 | 2.452 | 0.983 | 2.451 | 0.999 | yes |
| diffeq1 | 78 | 17.082 | 17.617 | 1.031 | 17.090 | 1.000 | 17.084 | 1.000 | yes |
| diffeq2 | 60 | 12.977 | 13.323 | 1.027 | 13.006 | 1.002 | 13.003 | 1.000 | yes |
| mcml | 162 | 46.357 | 46.452 | 1.002 | 46.367 | 1.000 | 46.343 | 0.999 | yes |
| mkDelayWorker32B | 50 | 8.430 | 8.463 | 1.004 | 8.460 | 1.004 | 8.414 | 0.995 | yes |
| mkPktMerge | 60 | 4.263 | 4.268 | 1.001 | 4.292 | 1.007 | 4.290 | 1.000 | yes |
| mkSMAdapter4B | 64 | 4.822 | 4.802 | 0.996 | 4.806 | 0.997 | 4.801 | 0.999 | yes |
| or1200 | 120 | 8.645 | 8.634 | 0.999 | 8.630 | 0.998 | 8.627 | 1.000 | yes |
| raygentop | 76 | 4.743 | 4.860 | 1.025 | 4.740 | 0.999 | 4.737 | 0.999 | yes |
| sha | 88 | 10.035 | 10.161 | 1.013 | 10.032 | 1.000 | 10.026 | 0.999 | yes |
| spree | 80 | 11.281 | 11.277 | 1.000 | 11.280 | 1.000 | 11.273 | 0.999 | yes |
| stereovision0 | 62 | 3.606 | 3.602 | 0.999 | 3.601 | 0.999 | 3.599 | 0.999 | yes |
| stereovision1 | 104 | 5.580 | 5.443 | 0.976 | 5.451 | 0.977 | 5.447 | 0.999 | yes |
| stereovision2 | 88 | 15.825 | 15.782 | 0.997 | 15.778 | 0.997 | 15.772 | 1.000 | yes |
| stereovision3 | 40 | 2.658 | 2.658 | 1.000 | 2.662 | 1.002 | 2.660 | 0.999 | yes |
| Geomean |  |  |  | 1.003 |  | 0.998 |  | 0.999 |  |

Wirelength at the relaxed width, in tiles of wire:

| Circuit | VPR | guide before | before / VPR | guide | guide / VPR |
| --- | --- | --- | --- | --- | --- |
| LU32PEEng | 1310007 | 1297548 | 0.990 | 1304833 | 0.996 |
| LU8PEEng | 325298 | 322552 | 0.992 | 324858 | 0.999 |
| arm_core | 184148 | 183390 | 0.996 | 184526 | 1.002 |
| bgm | 366218 | 365252 | 0.997 | 370151 | 1.011 |
| blob_merge | 66256 | 65834 | 0.994 | 67081 | 1.012 |
| boundtop | 3124 | 3150 | 1.008 | 3154 | 1.010 |
| ch_intrinsics | 1416 | 1485 | 1.049 | 1449 | 1.023 |
| diffeq1 | 8635 | 8726 | 1.011 | 8644 | 1.001 |
| diffeq2 | 9780 | 9799 | 1.002 | 9882 | 1.010 |
| mcml | 921573 | 918230 | 0.996 | 919519 | 0.998 |
| mkDelayWorker32B | 22576 | 22086 | 0.978 | 22082 | 0.978 |
| mkPktMerge | 13566 | 13395 | 0.987 | 13358 | 0.985 |
| mkSMAdapter4B | 17892 | 17939 | 1.003 | 18076 | 1.010 |
| or1200 | 43336 | 43678 | 1.008 | 43888 | 1.013 |
| raygentop | 18983 | 18933 | 0.997 | 18799 | 0.990 |
| sha | 15449 | 15463 | 1.001 | 15476 | 1.002 |
| spree | 12039 | 11963 | 0.994 | 12076 | 1.003 |
| stereovision0 | 55531 | 55592 | 1.001 | 55464 | 0.999 |
| stereovision1 | 114217 | 114462 | 1.002 | 114442 | 1.002 |
| stereovision2 | 422434 | 420551 | 0.996 | 419730 | 0.994 |
| stereovision3 | 783 | 777 | 0.992 | 756 | 0.966 |
| Geomean |  |  | 1.000 |  | 1.000 |

Runtime at the relaxed width, each ratio to VPR's time in the same run:

| Circuit | VPR, s | VPR's lookahead, s | VPR iterations | guide before, s | before / VPR | guide, 1 thread, s | guide's lookahead, s | guide iterations | / VPR | guide, threads, s | / VPR | Its delay, ns | VPR accepts it |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 24.36 | 4.87 | 22 | 10.67 | 0.48 | 11.05 | 5.21 | 14 | 0.45 | 10.46 | 0.43 | 76.111 | yes |
| LU8PEEng | 6.33 | 0.85 | 22 | 2.17 | 0.32 | 2.27 | 1.11 | 14 | 0.36 | 2.29 | 0.36 | 75.678 | yes |
| arm_core | 2.44 | 0.45 | 14 | 1.10 | 0.51 | 1.21 | 0.56 | 10 | 0.50 | 1.05 | 0.43 | 18.598 | yes |
| bgm | 7.29 | 0.89 | 20 | 2.37 | 0.32 | 2.83 | 1.10 | 14 | 0.39 | 3.12 | 0.43 | 20.465 | yes |
| blob_merge | 0.98 | 0.12 | 18 | 0.34 | 0.29 | 0.24 | 0.16 | 10 | 0.25 | 0.49 | 0.50 | 15.166 | yes |
| boundtop | 0.04 | 0.02 | 11 | 0.01 | 0.22 | 0.01 | 0.02 | 11 | 0.22 | 0.02 | 0.57 | 3.553 | yes |
| ch_intrinsics | 0.01 | 0.01 | 8 | 0.00 | 0.20 | 0.01 | 0.01 | 10 | 0.50 | 0.01 | 1.40 | 2.452 | yes |
| diffeq1 | 0.11 | 0.02 | 20 | 0.06 | 0.51 | 0.06 | 0.04 | 14 | 0.52 | 0.09 | 0.80 | 16.972 | yes |
| diffeq2 | 0.18 | 0.03 | 21 | 0.09 | 0.46 | 0.11 | 0.04 | 13 | 0.59 | 0.12 | 0.64 | 13.143 | yes |
| mcml | 17.31 | 5.04 | 21 | 6.95 | 0.40 | 7.34 | 5.22 | 14 | 0.42 | 9.01 | 0.52 | 46.367 | yes |
| mkDelayWorker32B | 0.45 | 0.30 | 15 | 0.15 | 0.33 | 0.13 | 0.45 | 13 | 0.28 | 0.18 | 0.41 | 8.473 | yes |
| mkPktMerge | 0.13 | 0.07 | 15 | 0.07 | 0.62 | 0.05 | 0.11 | 9 | 0.39 | 0.07 | 0.52 | 4.292 | yes |
| mkSMAdapter4B | 0.26 | 0.04 | 17 | 0.07 | 0.36 | 0.07 | 0.06 | 11 | 0.28 | 0.12 | 0.46 | 4.800 | yes |
| or1200 | 0.47 | 0.12 | 19 | 0.25 | 0.43 | 0.27 | 0.23 | 13 | 0.57 | 0.31 | 0.66 | 8.635 | yes |
| raygentop | 0.26 | 0.04 | 16 | 0.12 | 0.39 | 0.13 | 0.06 | 12 | 0.50 | 0.17 | 0.65 | 4.740 | yes |
| sha | 0.17 | 0.03 | 21 | 0.05 | 0.21 | 0.04 | 0.05 | 13 | 0.26 | 0.10 | 0.61 | 10.032 | yes |
| spree | 0.13 | 0.02 | 14 | 0.05 | 0.39 | 0.06 | 0.03 | 11 | 0.43 | 0.10 | 0.73 | 11.282 | yes |
| stereovision0 | 0.77 | 0.13 | 14 | 0.20 | 0.20 | 0.23 | 0.22 | 10 | 0.29 | 0.33 | 0.43 | 3.602 | yes |
| stereovision1 | 1.42 | 0.34 | 13 | 0.82 | 0.59 | 0.87 | 0.47 | 13 | 0.61 | 0.95 | 0.67 | 5.457 | yes |
| stereovision2 | 5.00 | 2.39 | 19 | 2.15 | 0.45 | 2.26 | 3.13 | 12 | 0.45 | 3.47 | 0.69 | 15.775 | yes |
| stereovision3 | 0.02 | 0.00 | 19 | 0.00 | 0.15 | 0.00 | 0.00 | 9 | 0.20 | 0.01 | 0.40 | 2.658 | yes |
| Geomean |  |  |  |  | 0.35 |  |  |  | 0.38 |  | 0.56 |  |  |

What this shows:

- **Channel width: `guide` needs 4.7% fewer tracks than VPR**, where it needed 1.4% more
  before, and 6.0% fewer on MCNC. It needs fewer on 11 circuits, by up to 18% (sha, 56
  against 68, and or1200, 76 against 92), as many on 8, and more on 2: blob_merge, 60 against
  58, and mkDelayWorker32B, 40 against 38. Against before, it needs fewer on 13 circuits and
  more on none. The largest circuits gain too: LU32PEEng needs 116 where it needed 122,
  against VPR's 124, and mcml 112 where it needed 126. blob_merge and mkDelayWorker32B still
  run out of their 50 iterations at VPR's width.
- **Delay: 0.2% shorter critical paths than VPR**, where they were 0.3% longer. 17 of 21 are
  within 0.5% of VPR's either way, and the three that were 2.5% to 3.1% slower, diffeq1,
  diffeq2 and raygentop, all with multipliers, are now within 0.2%. The slowest is mkPktMerge,
  0.7% slower, and the fastest stereovision1, 2.3% faster.
- **Wirelength: the same as VPR's**, a geomean of 1.000 as before, from 3.4% less on
  stereovision3 to 2.3% more on ch_intrinsics.
- **Runtime: `guide` takes 0.38 times VPR's time**, where it took 0.35, from 0.20 times on
  stereovision3 to 0.61 on stereovision1, and 0.45 times on the largest, LU32PEEng, 11.1 s
  against 24.4 s. It takes 11.8 iterations where it took 10.6, against VPR's 16.6, and 6% more
  expansions. With each tool's lookahead counted in, 0.59 times: `guide`'s lookahead is 1.1
  to 2.2 times as slow as VPR's, and on the smallest circuits takes longer than the routing.
- **Threads helped on the largest circuits only** here: 16 threads with batches of 32 took 0.95
  times as long as one thread on LU32PEEng and 0.87 on arm_core, but 1.45 times as long over
  all 21. Since, they take 0.56 times as long over all 21: see
  [threads](#threads-on-the-larger-circuits) below.
- **`guide`'s own timing analysis agrees with VPR's** to within 0.5%, and to 0.1% on 19 of 21.

#### Tuning the negotiation on both suites

`RrGraph::engine_options` was first tuned on MCNC alone, with a present factor growth of 1.8
and a history of one tile of wire. VPR's growth is 1.3, and its history is in effect a whole
wire. These were tuned again on both suites together, which changed three things: a growth of
1.5, a history of two tiles, and giving up early on widths that will not route.

**How.** Each suite's circuits were split in two by alternating through them in alphabetical
order, a tuning half and a held-out half, and options were chosen on the tuning halves only.
LU32PEEng and mcml were left out for time, and stereovision2 too, as one width of its search
took an hour and a half without giving up early. Each set of options ran the same search for
the narrowest channel as above and one route at the relaxed width, on graphs that VPR wrote
once and that every run then read, so that a run takes minutes. Delays here are `guide`'s own
analysis, its worst slack, which agrees with VPR's to within 0.1% on most circuits and 1.2% on
all, and wirelength is `guide`'s count, which agrees with VPR's on every circuit. Times are as
expansions, since up to twenty sets of options ran at once.

The rule for choosing was the narrowest channels on the tuning halves of both suites,
without longer critical paths than before on either or more than a few tenths of a percent
more wire, for at most about a tenth more expansions. All but expansions are geomeans of ratios to VPR's; tuning halves of 10 and
9 circuits, held-out halves of 10 and 9:

| Growth | History, tiles | Other | MCNC W, tuning | MCNC W, held out | Verilog W, tuning | Verilog W, held out | Delay, MCNC | Delay, Verilog | Wire, MCNC | Wire, Verilog | Expansions / default |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1.8 | 1 |  | 0.954 | 0.963 | 1.021 | 1.005 | 0.987 | 1.003 | 0.945 | 1.001 | 1.00 |
| 1.5 | 1 |  | 0.948 | 0.970 | 0.993 | 1.000 | 0.988 | 0.998 | 0.940 | 0.996 | 1.09 |
| 1.4 | 1 |  | 0.934 | 0.956 | 0.976 | 0.987 | 0.988 | 0.997 | 0.937 | 0.993 | 1.14 |
| 1.3 | 1 |  | 0.926 | 0.964 | 0.996 | 0.992 | 0.986 | 0.998 | 0.934 | 0.991 | 1.21 |
| 1.3 | 1 | first present 0 | 0.929 | 0.957 | 0.983 | 0.988 | 0.986 | 0.999 | 0.938 | 0.994 | 1.38 |
| 1.3 | 1 | initial present 1 | 0.931 | 0.966 | 1.004 | 0.994 | 0.986 | 0.998 | 0.941 | 0.999 | 1.00 |
| 1.3 | 1 | bound weight 1.2 | 0.934 | 0.956 | 0.982 | 0.992 | 0.989 | 1.000 | 0.933 | 0.994 | 1.14 |
| 1.3 | 0.5 |  | 0.955 | 1.001 | 1.006 | 1.017 | 0.987 | 1.000 | 0.932 | 0.989 | 1.27 |
| 1.8 | 1.5 |  | 0.948 | 0.959 | 0.974 | 0.981 | 0.986 | 1.004 | 0.948 | 1.003 | 0.98 |
| 1.8 | 2 |  | 0.931 | 0.960 | 0.932 | 0.974 | 0.985 | 1.006 | 0.956 | 1.010 | 0.98 |
| 1.8 | 3 |  | 0.930 | 0.959 | 0.943 | 0.975 | 0.986 | 1.001 | 0.961 | 1.014 | 0.99 |
| 1.8 | 4 |  | 0.944 | 0.973 | 0.957 | 0.987 | 0.986 | 0.999 | 0.970 | 1.021 | 1.01 |
| 1.6 | 1.5 |  | 0.929 | 0.969 | 0.967 | 0.985 | 0.986 | 1.001 | 0.943 | 0.999 | 1.02 |
| 1.6 | 2 |  | 0.915 | 0.970 | 0.944 | 0.976 | 0.986 | 0.997 | 0.951 | 1.007 | 1.02 |
| 1.5 | 1.5 |  | 0.932 | 0.960 | 0.958 | 0.991 | 0.987 | 1.000 | 0.942 | 0.998 | 1.06 |
| **1.5** | **2** |  | **0.915** | **0.965** | **0.938** | **0.983** | **0.984** | **0.997** | **0.947** | **1.001** | **1.05** |
| 1.5 | 2.5 |  | 0.930 | 0.963 | 0.930 | 0.976 | 0.984 | 1.001 | 0.953 | 1.008 | 1.06 |
| 1.5 | 3 |  | 0.914 | 0.966 | 0.935 | 0.974 | 0.985 | 0.996 | 0.954 | 1.008 | 1.06 |
| 1.5 | 2 | initial present 1 | 0.938 | 0.957 | 0.939 | 0.978 | 0.986 | 1.004 | 0.953 | 1.006 | 0.90 |
| 1.5 | 2 | 2 timing iterations | 0.915 | 0.965 | 0.938 | 0.983 | 0.984 | 0.997 | 0.947 | 1.001 | 1.49 |
| 1.4 | 2 |  | 0.927 | 0.946 | 0.941 | 0.980 | 0.986 | 0.999 | 0.947 | 1.001 | 1.11 |
| 1.3 | 1.5 |  | 0.907 | 0.965 | 0.950 | 0.984 | 0.985 | 0.996 | 0.937 | 0.993 | 1.17 |
| 1.3 | 2 |  | 0.906 | 0.949 | 0.961 | 0.973 | 0.986 | 0.996 | 0.943 | 0.997 | 1.16 |
| 1.3 | 3 |  | 0.907 | 0.946 | 0.944 | 0.970 | 0.986 | 0.996 | 0.953 | 1.007 | 1.18 |
| 1.3 | 4 |  | 0.904 | 0.952 | 0.933 | 0.968 | 0.986 | 0.997 | 0.963 | 1.016 | 1.20 |

- **History matters more than growth.** Two tiles instead of one, at the old growth of 1.8,
  took Verilog's tuning half from 1.021 to 0.932 of VPR's width for no more expansions, but
  gave 0.3% longer critical paths and 1% more wire there. More history goes on narrowing
  channels and adds more wire: four tiles gave 2% more on Verilog.
- **Growth then trades time for wire and delay.** At two tiles, 1.5 gave the narrowest
  channels on the tuning halves together, critical paths as short as any, and the wire of
  before, for 5% more expansions. 1.3, VPR's, gave 0.906 of VPR's width on MCNC's tuning half
  but 0.961 on Verilog's, for 16% more expansions.
- **Held out, the gain is on Verilog only.** The chosen options give 0.983 of VPR's width on
  Verilog's held-out half, against 1.005, but 0.965 on MCNC's, against 0.963: the gain on
  MCNC's tuning half, 0.954 to 0.915, is partly the choice fitting those circuits. Every
  set of options with a history of two tiles or more narrowed the held-out halves by less
  than the tuning halves, and MCNC's by 1.8% at most. Delay and wire held out moved as on
  the tuning halves.
- **What did not help.** A first present factor of 0, as VPR has it, took 15% more
  expansions than 0.5 at a growth of 1.3, for channels about as wide. Starting the second
  iteration at 1 rather than 0.5 took 14% fewer expansions with the chosen options, but gave
  longer critical paths and more wire on Verilog. A weight of 1.2 on the bound, rather than
  1.1, gave longer critical paths on both. Rerouting critical nets for two more
  iterations after the first legal result (`timing_iterations`) never found a better result
  than the first, on any circuit, for 42% more expansions.

**Giving up early.** VPR's search for the narrowest channel allows 50 iterations at each
width, as here, but its routing failure predictor (`--routing_failure_predictor safe`) stops
a width once the trend of its overused nodes says it would take more than three times that,
if more than 100 nodes are overused. `EngineOptions::give_up_factor` does the same. Most of a
search's time goes on the widths that fail: 84% of its expansions with the chosen options,
whose failing widths all ran their 50 iterations. Over the same 38 circuits:

| Give up at | Overused, at least | Searches' expansions / not giving up | Widths changed |
| --- | --- | --- | --- |
| Never |  | 1 |  |
| 3 times 50 iterations, VPR's `safe` | 100 | 0.73 | none |
| 1.5 times, VPR's `aggressive` | 100 | 0.67 | none |
| **1 times** | **100** | **0.57** | **none** |
| 0.75 times | 100 | 0.49 | none |
| 1 times | 10 | 0.49 | apex2 and ex1010, 2 tracks wider |

A factor of 1 is in `engine_options`, so that a run gives up only when its trend says it would
not finish within the limit. 0.75 saved more and changed no width here either, but gives up on
runs predicted to finish within the limit, and runs that did route took up to 50 iterations.
Below 100 overused nodes, it gave up on runs that would have routed. On stereovision2, which
the table leaves out, the search tried width 54, which took 5.6 billion expansions and an hour
and a half in 50 iterations, and 0.26 billion and six minutes giving up after 13. Both searches
found 60, where VPR finds 68.

#### Threads on the larger circuits

Batches of 32 on 16 threads used to be slower than one thread with batches of 1 on most
circuits, and are now about twice as fast on the larger ones. "Before" is `6583518`, and
"after" the commits that follow it, with the tuned options throughout. Times are the sum of
`guide`'s route times over each suite, timing-driven, the better of two runs at the relaxed
width, on a machine shared with other work and with other measurements running alongside:

| Threads, batch size | MCNC (20), s | / before, 1 thread | Verilog (21), s | / before, 1 thread | Wire | Delay |
| --- | --- | --- | --- | --- | --- | --- |
| 1, 1 | 1.68 → 1.68 | 1.00 → 1.00 | 29.0 → 29.1 | 1.00 → 1.00 | | |
| 1, 32 | 2.62 → 2.08 | 1.56 → 1.24 | 36.0 → 33.9 | 1.24 → 1.17 | +0.39% → +0.14% | +0.11% → +0.15% |
| 4, 32 | 2.95 → 1.48 | 1.75 → 0.88 | 32.4 → 18.1 | 1.12 → 0.62 | | |
| 16, 32 | 2.94 → 1.41 | 1.75 → 0.84 | 32.4 → 16.2 | 1.12 → 0.56 | | |
| 64, 32 | 2.92 → 1.55 | 1.74 → 0.92 | 32.9 → 16.8 | 1.13 → 0.58 | | |

Wire and delay are the means over all 41 circuits against batches of 1, the same at every
thread count, and the iterations went from 11.1 at batches of 1 to 11.3 before and 11.5 after.
Every route was legal. The MCNC circuits route in 15 to 300 ms, which leaves threads little to
share, and the geometric mean over them is 0.92 at 16 threads, against 1.87 before; over the
Verilog circuits it is 0.77, against 1.62.

On the larger circuits, in seconds:

| Circuit | Before: 1 thread, batch 1 | 16 threads, batch 32 | After: 1 thread, batch 1 | 1 thread, batch 32 | 4 threads | 16 threads | 64 threads |
| --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 12.29 | 10.77 | 11.86 | 13.49 | 7.05 | 6.17 | 6.35 |
| mcml | 7.06 | 8.89 | 7.34 | 7.92 | 4.94 | 4.73 | 4.79 |
| LU8PEEng | 2.22 | 2.24 | 2.05 | 2.49 | 1.21 | 1.04 | 1.10 |
| bgm | 2.30 | 3.02 | 2.59 | 3.49 | 1.41 | 1.09 | 1.12 |
| stereovision2 | 2.21 | 3.27 | 2.27 | 2.66 | 1.51 | 1.33 | 1.43 |
| arm_core | 0.92 | 1.00 | 0.94 | 1.22 | 0.56 | 0.52 | 0.54 |
| stereovision1 | 0.76 | 1.05 | 0.67 | 0.90 | 0.46 | 0.43 | 0.47 |
| clma | 0.21 | 0.35 | 0.24 | 0.28 | 0.18 | 0.18 | 0.18 |

Beyond 16 threads nothing more comes: a batch of 32 has no work for more than 32, a batch is as
slow as its biggest search, and searches take more CPU time the more threads share the
machine's caches.

Larger batches now pay. At 16 threads, after, in seconds and, below, as expansions, wire,
delay and iterations against batches of 1 on one thread:

| Circuit | 1 thread, batch 1 | Batch 4 | 8 | 16 | 32 | 64 | 128 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 11.77 | 8.77 | 7.83 | 6.49 | 6.02 | 5.71 | 5.42 |
| mcml | 7.49 | 6.19 | 5.54 | 5.48 | 4.75 | 4.46 | 4.37 |
| LU8PEEng | 2.36 | 1.63 | 1.44 | 1.24 | 1.00 | 0.93 | 0.83 |
| bgm | 2.52 | 2.17 | 1.67 | 1.39 | 1.12 | 0.94 | 0.93 |
| stereovision2 | 2.31 | 2.05 | 1.85 | 1.52 | 1.33 | 1.17 | 1.08 |
| arm_core | 1.13 | 0.83 | 0.69 | 0.59 | 0.52 | 0.45 | 0.43 |
| clma | 0.30 | 0.23 | 0.22 | 0.20 | 0.17 | 0.17 | 0.15 |

| Circuit | Batch 4 | 8 | 16 | 32 | 64 | 128 |
| --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 1.06, +0.08%, +0.04%, 14 | 1.12, +0.14%, +0.03%, 15 | 1.19, +0.23%, +0.02%, 13 | 1.25, +0.30%, +0.02%, 16 | 1.31, +0.41%, +0.01%, 14 | 1.35, +0.35%, +0.00%, 14 |
| mcml | 1.07, -0.01%, +0.03%, 13 | 1.13, -0.01%, +0.03%, 13 | 1.20, +0.06%, +0.02%, 13 | 1.23, -0.07%, +0.02%, 14 | 1.28, +0.06%, +0.02%, 13 | 1.32, +0.14%, +0.03%, 13 |
| LU8PEEng | 1.06, -0.01%, +0.02%, 15 | 1.14, +0.27%, +0.00%, 14 | 1.22, +0.04%, -0.03%, 15 | 1.30, +0.17%, -0.02%, 14 | 1.34, +0.15%, -0.01%, 14 | 1.40, +0.19%, +0.02%, 14 |
| bgm | 1.15, -0.16%, +0.05%, 16 | 1.19, +0.25%, +0.06%, 13 | 1.31, +0.16%, +0.03%, 14 | 1.46, +0.10%, +0.10%, 13 | 1.62, +0.35%, +0.04%, 12 | 1.73, +0.56%, +0.04%, 15 |
| stereovision2 | 1.11, +0.10%, -0.01%, 13 | 1.18, +0.12%, -0.01%, 13 | 1.24, +0.09%, -0.01%, 12 | 1.32, +0.12%, +0.02%, 14 | 1.37, +0.12%, +0.01%, 12 | 1.40, +0.09%, -0.01%, 14 |
| arm_core | 1.05, -0.06%, +0.03%, 10 | 1.08, -0.37%, +0.02%, 11 | 1.15, -0.30%, +0.03%, 11 | 1.19, +0.05%, +0.01%, 11 | 1.22, -0.30%, +0.03%, 11 | 1.30, -0.01%, +0.02%, 11 |
| clma | 1.09, +0.09%, +0.01%, 10 | 1.16, +0.79%, -0.03%, 12 | 1.23, +0.47%, -0.05%, 11 | 1.23, +0.00%, -0.03%, 10 | 1.27, -0.28%, -0.02%, 11 | 1.32, +0.05%, -0.02%, 10 |

Batches of 1 take 14, 14, 14, 14, 12, 10 and 13 iterations. So on 16 threads batches of 128
are 1.7 to 2.8 times as fast as one thread with batches of 1 on these circuits, and batches
of 32 are 1.6 to 2.4 times as fast, for up to 0.56% more wire, on bgm at 128, and the same
delays to 0.1%. This was measured on the relaxed width only: `RrGraph::engine_options` still
has batches of 1, and the minimum-width searches below used batches of 32.

The minimum-width searches gain as much or more, as their failing widths take many more
iterations. Each circuit was routed at every width that its tuned search tried, at most 50
iterations each and giving up early as tuned, and the times summed, in seconds. LU32PEEng
and mcml are left out, as a width took up to half an hour on one thread, and stereovision2
is apart, as it is most of the time. Four ran at once; "before" has no 4 or 64 threads:

| Threads, batch size | MCNC (20 circuits, 78 widths) | Verilog (18, 62) | stereovision2 (6) |
| --- | --- | --- | --- |
| 1, 1 | 31.0 → 31.0 | 389 → 389 | 1227 → 1271 |
| 1, 32 | 48.3 → 37.0 | 569 → 558 | 1571 → 1791 |
| 4, 32 | → 24.0 | → 219 | → 573 |
| 16, 32 | 39.5 → 24.1 | 278 → 178 | 651 → 334 |
| 64, 32 | → 25.7 | → 179 | → 321 |

Batches of 32 move the narrowest width that routes, both ways, as they did before. After, of
the MCNC circuits, apex2, clma, ex5p, pdc and seq route 2 tracks narrower, alu4, apex4 and
diffeq need 2 more, s298 and ex1010 4 more, and dsip routes at none of the widths tried,
which stop at its width at batches of 1. Of the Verilog ones, diffeq2, mkSMAdapter4B, or1200
and stereovision1 route 2 narrower, and sha and stereovision2 need 2 more. Before, it was 3
narrower and 4 wider on MCNC, and 3 narrower and 5 wider on Verilog, with blob_merge routing
at none of the widths tried and stereovision2 needing 6 more. A search in batches would try
other widths than these, so its time would differ too.

##### Where the time went

Measured with timers in the engine, since `perf` does not run on this machine, on LU8PEEng at
the relaxed width without timing, where batches of 32 took 3.4 s on one thread and 3.2 s on 16,
against 2.2 s for batches of 1:

- **A few nets of hundreds of terminals.** The 22 routes of nets of 256 terminals or more
  took 865 ms of 3.2 s, ten times as long per expansion as nets of 2 or 3 terminals. A search's
  heuristic is the lowest bound over every terminal still to connect, and every node of the
  tree is a source of the next search, so such a net asks for bounds on the order of `n`
  cubed times. They also held up their batch, which is as slow as its slowest search: on 2 to
  16 threads the longest search of each batch added up to 2.4 to 2.6 s of 3.0 s of batches.
  And they were dropped and routed again: once their heuristics were kept, as below, 0.48 s
  of 0.86 s of searches thrown away on one thread was 26 routes of nets of 128 terminals or
  more.
- **Batches that ran short.** A route dropped for a conflict, which is a fifth of them on
  these graphs, held up every route after it, so that the next batch could only take in as
  many new nets as had been committed. Batches of 32 held 5.6 new nets on average on
  LU8PEEng, and 4.1 on stereovision2 with timing, which is what left 16 threads with nothing
  to do.
- **Starting threads for each batch**, at up to 50 us a batch over 8105 batches, or 0.4 s.
- **CPU time per search grew with threads**: 1.2 to 1.6 times as much with 8 threads in one
  8-core group of this machine, and 2.2 times with 16 spread over it. That is caches.
- Not the search state of each thread, which a generation counter resets in no time; not the
  table, which was never copied, only shared; not the map lookahead or timing data, which
  are read-only and shared.

What changed, in the order it was found:

- Searches of a tree keep the heuristics that cannot have changed since the last one, which
  only the bounds to the terminals just connected can tell, and give the same results. That
  took the nets of 256 terminals from 865 ms to 575 ms on one thread, and the run from 2.3 s
  to 1.8 s, without timing. With timing, critical connections are routed one at a time and
  this does little.
- A net of 64 terminals or more only routes first in its batch, where it cannot be dropped:
  on one thread with batches of 32, without timing, 0.40 s of searches were thrown away
  where 0.93 s were.
- Threads are started once an iteration, keep their worker, and are lent the table for a
  batch through a lock that only the committing thread writes to, between batches.
- A dropped net routes again first in the next batch, after the rest of its batch, so that
  batches stay full: 1917 batches on LU8PEEng where there were 10093, and 3420 on
  stereovision2 where there were 27480. That gives up the order of the nets within a batch,
  which costs dense grids of crossing nets more iterations: the [engine's
  documentation](../src/engine.rs) has the numbers.

#### What the Verilog benchmarks took

The MCNC circuits had none of what this architecture adds, and `guide` could not route the
first of these circuits, ch_intrinsics, at all. Four things needed fixing, in `2bc0139`:

- **Blocks over several tiles.** VPR 9 puts the `SOURCE` or `SINK` of a pin class on the tile
  of its pins, not on the block's root tile, so a memory's address pins have their `SINK`s on
  six different tiles. `resolve_nets` looked only at the root, and found no node for most
  pins of a memory or multiplier. It now looks at every tile of the block.
- **Constant nets.** VPR does not route nets that carry a constant (`--constant_net_method
  global`), which it finds by inference through LUTs and registers (`--const_gen_inference
  comb_seq`). `guide` routed them, 1770 of them on mkDelayWorker32B, and VPR accepted the
  routes but counted less wire than `guide` did. `fpga::constant_nets` follows VPR's
  inference, `NetOptions::constant_nets` leaves them unrouted, and the count agrees with VPR's
  on every circuit, as does the wirelength.
- **The primitives VPR makes up for a `class`.** Every `class="lut"` becomes a primitive
  called `lut` and every `class="memory"` one called `memory_slice`, whatever its parent, and
  `read_architecture` kept the last of each. So every LUT took `lut6`'s delay, 261 ps, where
  `lut5` has 235 ps and `lut4` 195 ps, and every memory the model of the last memory mode, a
  dual-port one, whose port names don't match a single-port memory's, so its setup checks
  disappeared. On ch_intrinsics, `guide`'s analysis made the critical path 2.18 ns, which VPR
  timed at 2.88 ns. `read_architecture` now keeps these under `lut5.lut` and so on as well,
  and `build_timing` looks them up with `PackedNetlist::primitive_parent`: the two analyses
  now agree to within 0.2% on every circuit but one, mkDelayWorker32B, at 0.5%. The names of
  a memory's interconnect, `direct{n}` and `direct{n}_{slice}`, were also wrong, which only
  cost a warning, since VPR gives them no delay.

No rule like MCNC's one output pin per block showed up: this architecture's cluster outputs,
memories and multipliers are not equivalent, so each output pin has its own `SOURCE`, and VPR
accepted every route of `guide`'s, carry chains included, which take the graph's direct
connections from one cluster's `cout` to the next one's `cin` like any other edge.

#### What the Verilog benchmarks do not show

- **Titan23 was not run.** Most of its circuits are larger than any here, several times so at
  the top, they need a large download from outside VTR, and their Stratix IV architecture has
  two wire lengths, many more switch types and blocks with equivalent pins that this
  comparison has not met. VPR's own flow routes them at a fixed width of 300 rather than
  searching for a minimum, so the methodology here would have to change too. The largest
  circuit here, LU32PEEng, is still short of most of them.
- Synthesis is Odin II and ABC, as the `_odin` variant of VTR's test has it, where VTR's
  default flow now uses Yosys and Parmys, which was not built. Synthesized differently, the
  circuits would place and route differently, for both routers.
- One placement per circuit, from one seed, and one architecture, as for MCNC.
- `RrGraph::engine_options` was [tuned on both suites](#tuning-the-negotiation-on-both-suites),
  on half of each, so these results are not independent of it. The held-out halves gained
  less than the tuning halves, and on MCNC nothing in channel width.

## FPGA placement against VPR

This is the comparison that M8's exit criterion asks for, "FPGA placement competitive with VPR
after routing": `guide`'s placer against VPR's on the same packing, on both suites of the
routing comparisons above, with both placements routed by both routers, and VPR checking every
placement and route of `guide`'s.

Measured on 2026-11-10 with rustc 1.97.1, on the same shared machine, at `24d4e8b`, the commit
that adds the placer.

### How

- **VPR, the circuits and the architectures** are those of the routing comparisons: the 20 MCNC
  circuits on `k6_N10_40nm.xml`, and the 21 Verilog circuits on
  `k6_frac_N10_frac_chain_mem32K_40nm.xml`, with carry chains, multipliers and memories.
- **The packing** is VPR's, with seed 1, and both placers place the same `.net` file. VPR's
  placement is the one it made with that packing for the routing comparisons, with its
  defaults and seed 1: VPR 9 starts from
  a placement that puts blocks near their neighbours' centroid, anneals from a cold
  temperature with `--inner_num 0.5`, picks among seven kinds of move with a learning agent,
  and weighs wire against timing by its `criticality_timing` cost.
- **`guide`'s placement** is `examples/vtr_place.rs` with its defaults, one thread:
  `fpga::build_place_problem` on the device of the graph that VPR wrote at the relaxed width
  below, which is the device VPR sized for the circuit, and `fpga::place_design` with
  `PlacerOptions::default()`: analytic global placement and legalization, then annealing that
  starts cold with `inner_num` 1, with VPR's cost, criticalities from `guide`'s timing analysis
  each temperature and delays from `fpga::PlaceDelayModel`. Constant and clock nets are left
  out, as VPR leaves them out.
- **Legality** is VPR's: it loads `guide`'s placement with every block fixed
  (`--fix_clusters`), which goes through its placement consistency check, carry chains
  included, and it writes the placement back unchanged. It then routes it too.
- **Placement measures** are VPR's for both placements: its `BB estimate of min-dist
  (placement) wire length`, which `fpga::PlaceProblem::bb_estimate` computes to the same
  number on every placement of both suites, and its `estimated critical path delay`, from its
  own delay model. Times are VPR's `# Placement took`, from the run that packed and placed for
  the routing comparisons, and the time inside `fpga::place_design`. Neither counts reading
  files or building its delay model, which took VPR 0.02 s to 35 s, and `guide` 0.001 s to
  1.6 s.
- **Routing** is as in the routing comparisons, on each placement: each router's minimum
  channel width, by VPR's binary search and by `guide`'s from VPR's width on that placement;
  and at one relaxed width for both placements, 1.3 times VPR's minimum on VPR's placement,
  the critical-path delay by VPR's analysis and the wirelength by VPR's count.

```text
scripts/vtr/build-vpr.sh DIR                                 # as for routing
FROM=WORK scripts/vtr/place.sh DIR PWORK                     # MCNC, reusing VPR's packing
FROM=WORK SUITE=verilog BLIF=BLIF scripts/vtr/place.sh DIR PWORK2
python3 scripts/vtr/place_tables.py PWORK                    # the tables below
```

### Results

**Every placement is legal**: VPR loaded all 41 of `guide`'s with every block fixed, found them
consistent and routed them, and all of `guide`'s routes of them, at the minimum and at the
relaxed width, passed VPR's checks. Ratios are of `guide`'s placement over VPR's, so below 1 is
better for `guide`, and the last row is their geometric mean.

#### MCNC

Placement, as VPR measures both:

| Circuit | Blocks | Grid | VPR BB | guide BB | / VPR | VPR est. CPD, ns | guide est. CPD, ns | / VPR | VPR, s | guide, s | / VPR | Chains | VPR accepts guide's |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 106 | 13 | 5320 | 5170 | 0.972 | 4.98964 | 5.13851 | 1.030 | 0.080 | 0.142 | 1.775 | 0 | yes |
| apex2 | 126 | 14 | 8242 | 7969 | 0.967 | 5.81286 | 5.92692 | 1.020 | 0.110 | 0.236 | 2.145 | 0 | yes |
| apex4 | 105 | 13 | 6734 | 6725 | 0.999 | 5.30224 | 5.74674 | 1.084 | 0.100 | 0.129 | 1.290 | 0 | yes |
| bigkey | 92 | 16 | 4499 | 3259 | 0.724 | 3.19105 | 3.44405 | 1.079 | 0.270 | 0.294 | 1.089 | 0 | yes |
| clma | 435 | 23 | 32117 | 31845 | 0.992 | 8.66055 | 8.61265 | 0.994 | 1.140 | 1.763 | 1.546 | 0 | yes |
| des | 102 | 18 | 5227 | 4797 | 0.918 | 4.37046 | 4.75988 | 1.089 | 0.260 | 0.138 | 0.531 | 0 | yes |
| diffeq | 102 | 13 | 3921 | 3830 | 0.977 | 6.43054 | 6.41491 | 0.998 | 0.120 | 0.146 | 1.217 | 0 | yes |
| dsip | 93 | 16 | 4931 | 4095 | 0.830 | 3.18883 | 3.44405 | 1.080 | 0.310 | 0.301 | 0.971 | 0 | yes |
| elliptic | 242 | 18 | 12132 | 11972 | 0.987 | 7.4606 | 7.91761 | 1.061 | 0.540 | 0.812 | 1.504 | 0 | yes |
| ex1010 | 322 | 20 | 27552 | 27318 | 0.992 | 7.05556 | 7.38685 | 1.047 | 0.520 | 1.124 | 2.162 | 0 | yes |
| ex5p | 98 | 12 | 4942 | 4777 | 0.967 | 4.47718 | 4.71557 | 1.053 | 0.080 | 0.170 | 2.125 | 0 | yes |
| frisc | 251 | 18 | 15146 | 14976 | 0.989 | 10.0229 | 10.196 | 1.017 | 0.470 | 0.844 | 1.796 | 0 | yes |
| misex3 | 100 | 12 | 5175 | 5101 | 0.986 | 4.84801 | 5.20847 | 1.074 | 0.070 | 0.153 | 2.186 | 0 | yes |
| pdc | 332 | 21 | 25399 | 25607 | 1.008 | 6.92036 | 7.20019 | 1.040 | 0.720 | 1.417 | 1.968 | 0 | yes |
| s298 | 84 | 12 | 4089 | 4027 | 0.985 | 7.44269 | 7.40466 | 0.995 | 0.050 | 0.080 | 1.600 | 0 | yes |
| s38417 | 397 | 22 | 14984 | 13542 | 0.904 | 5.80932 | 5.96365 | 1.027 | 0.710 | 1.125 | 1.585 | 0 | yes |
| s38584.1 | 360 | 21 | 14456 | 14427 | 0.998 | 5.23191 | 5.59858 | 1.070 | 0.960 | 1.363 | 1.420 | 0 | yes |
| seq | 112 | 13 | 7217 | 7238 | 1.003 | 4.98507 | 5.34932 | 1.073 | 0.120 | 0.293 | 2.442 | 0 | yes |
| spla | 265 | 19 | 17307 | 17005 | 0.983 | 6.63208 | 6.48149 | 0.977 | 0.430 | 0.993 | 2.309 | 0 | yes |
| tseng | 112 | 13 | 3158 | 3054 | 0.967 | 6.15771 | 6.31173 | 1.025 | 0.130 | 0.193 | 1.485 | 0 | yes |
| Geomean |  |  |  |  | 0.955 |  |  | 1.041 |  |  | 1.571 |  |  |

Routed by VPR's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 40 | 40 | 1.000 | 52 | 5.31783 | 5.11503 | 0.962 | 7291 | 6935 | 0.951 | yes |
| apex2 | 54 | 56 | 1.037 | 70 | 5.80687 | 5.58316 | 0.961 | 12151 | 11855 | 0.976 | yes |
| apex4 | 56 | 56 | 1.000 | 72 | 5.38635 | 5.43986 | 1.010 | 10212 | 9726 | 0.952 | yes |
| bigkey | 34 | 34 | 1.000 | 44 | 3.30692 | 3.2681 | 0.988 | 6415 | 4968 | 0.774 | yes |
| clma | 72 | 72 | 1.000 | 94 | 8.69463 | 8.38633 | 0.965 | 44365 | 44880 | 1.012 | yes |
| des | 34 | 34 | 1.000 | 44 | 4.46945 | 4.41387 | 0.988 | 6744 | 5989 | 0.888 | yes |
| diffeq | 30 | 32 | 1.067 | 40 | 6.09481 | 6.09441 | 1.000 | 5218 | 5137 | 0.984 | yes |
| dsip | 34 | 34 | 1.000 | 44 | 3.17889 | 3.43832 | 1.082 | 7198 | 6570 | 0.913 | yes |
| elliptic | 52 | 48 | 0.923 | 68 | 7.58148 | 7.60222 | 1.003 | 16865 | 16256 | 0.964 | yes |
| ex1010 | 86 | 86 | 1.000 | 112 | 6.89706 | 6.93223 | 1.005 | 40817 | 40109 | 0.983 | yes |
| ex5p | 44 | 46 | 1.045 | 58 | 4.62135 | 4.71745 | 1.021 | 6964 | 6567 | 0.943 | yes |
| frisc | 58 | 56 | 0.966 | 76 | 10.0072 | 9.76625 | 0.976 | 21617 | 21172 | 0.979 | yes |
| misex3 | 46 | 44 | 0.957 | 60 | 4.82071 | 4.94848 | 1.027 | 6920 | 6864 | 0.992 | yes |
| pdc | 72 | 72 | 1.000 | 94 | 7.06044 | 7.14236 | 1.012 | 36688 | 36866 | 1.005 | yes |
| s298 | 40 | 40 | 1.000 | 52 | 7.26292 | 6.91572 | 0.952 | 5886 | 5717 | 0.971 | yes |
| s38417 | 34 | 30 | 0.882 | 44 | 5.80047 | 5.74792 | 0.991 | 18574 | 16879 | 0.909 | yes |
| s38584.1 | 40 | 38 | 0.950 | 52 | 5.05609 | 5.16437 | 1.021 | 19108 | 19033 | 0.996 | yes |
| seq | 54 | 54 | 1.000 | 70 | 4.87201 | 5.12259 | 1.051 | 10428 | 10247 | 0.983 | yes |
| spla | 60 | 60 | 1.000 | 78 | 6.52939 | 6.24003 | 0.956 | 25425 | 24652 | 0.970 | yes |
| tseng | 26 | 24 | 0.923 | 34 | 5.71256 | 5.66566 | 0.992 | 4210 | 4117 | 0.978 | yes |
| Geomean |  |  | 0.987 |  |  |  | 0.998 |  |  | 0.955 |  |

Routed by guide's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 38 | 36 | 0.947 | 52 | 4.80677 | 5.05782 | 1.052 | 6840 | 6426 | 0.939 | yes |
| apex2 | 52 | 58 | 1.115 | 70 | 5.77742 | 5.60368 | 0.970 | 12015 | 11403 | 0.949 | yes |
| apex4 | 54 | 56 | 1.037 | 72 | 5.28066 | 5.44193 | 1.031 | 9338 | 9098 | 0.974 | yes |
| bigkey | 34 | 34 | 1.000 | 44 | 3.26427 | 3.25733 | 0.998 | 6328 | 4797 | 0.758 | yes |
| clma | 66 | 68 | 1.030 | 94 | 8.6618 | 8.20531 | 0.947 | 42623 | 42152 | 0.989 | yes |
| des | 34 | 32 | 0.941 | 44 | 4.46045 | 4.43248 | 0.994 | 6580 | 5968 | 0.907 | yes |
| diffeq | 30 | 30 | 1.000 | 40 | 6.19929 | 5.70597 | 0.920 | 4749 | 4667 | 0.983 | yes |
| dsip | 34 | 34 | 1.000 | 44 | 3.17556 | 3.41152 | 1.074 | 6918 | 6098 | 0.881 | yes |
| elliptic | 48 | 46 | 0.958 | 68 | 7.5485 | 7.58445 | 1.005 | 15349 | 15044 | 0.980 | yes |
| ex1010 | 82 | 80 | 0.976 | 112 | 6.82256 | 6.74024 | 0.988 | 38710 | 38742 | 1.001 | yes |
| ex5p | 44 | 42 | 0.955 | 58 | 4.55607 | 4.47229 | 0.982 | 6771 | 6492 | 0.959 | yes |
| frisc | 54 | 52 | 0.963 | 76 | 9.81195 | 9.55312 | 0.974 | 19599 | 18948 | 0.967 | yes |
| misex3 | 42 | 42 | 1.000 | 60 | 4.78566 | 4.92125 | 1.028 | 6791 | 6549 | 0.964 | yes |
| pdc | 64 | 68 | 1.062 | 94 | 7.05184 | 7.05985 | 1.001 | 34460 | 34905 | 1.013 | yes |
| s298 | 40 | 38 | 0.950 | 52 | 7.4939 | 6.77407 | 0.904 | 5210 | 5092 | 0.977 | yes |
| s38417 | 32 | 28 | 0.875 | 44 | 5.51245 | 5.4824 | 0.995 | 17595 | 15840 | 0.900 | yes |
| s38584.1 | 38 | 36 | 0.947 | 52 | 4.87876 | 5.10715 | 1.047 | 18080 | 18226 | 1.008 | yes |
| seq | 52 | 50 | 0.962 | 70 | 4.82294 | 4.97785 | 1.032 | 9654 | 9844 | 1.020 | yes |
| spla | 56 | 52 | 0.929 | 78 | 6.48223 | 6.2356 | 0.962 | 23896 | 23256 | 0.973 | yes |
| tseng | 26 | 26 | 1.000 | 34 | 5.68422 | 5.66455 | 0.997 | 4077 | 3888 | 0.954 | yes |
| Geomean |  |  | 0.981 |  |  |  | 0.994 |  |  | 0.953 |  |

#### VTR's Verilog benchmarks

Blocks are CLBs, then multipliers and memories, and chains are the carry chains placed as
macros.

Placement, as VPR measures both:

| Circuit | Blocks | Grid | VPR BB | guide BB | / VPR | VPR est. CPD, ns | guide est. CPD, ns | / VPR | VPR, s | guide, s | / VPR | Chains | VPR accepts guide's |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 7600 / 32 / 167 | 103 | 1043923 | 1046901 | 1.003 | 76.166 | 75.8237 | 0.996 | 341.290 | 232.415 | 0.681 | 166 | yes |
| LU8PEEng | 2232 / 8 / 44 | 57 | 238874 | 237688 | 0.995 | 75.9908 | 77.2229 | 1.016 | 33.930 | 16.561 | 0.488 | 70 | yes |
| arm_core | 1023 / 0 / 24 | 39 | 141706 | 143610 | 1.013 | 18.1898 | 18.2613 | 1.004 | 8.490 | 9.332 | 1.099 | 10 | yes |
| bgm | 2786 / 11 / 0 | 63 | 250115 | 233987 | 0.936 | 20.6448 | 19.8352 | 0.961 | 30.110 | 20.498 | 0.681 | 38 | yes |
| blob_merge | 619 / 0 / 0 | 31 | 45944 | 47786 | 1.040 | 15.2449 | 15.479 | 1.015 | 2.520 | 2.853 | 1.132 | 0 | yes |
| boundtop | 97 / 0 / 0 | 14 | 1935 | 1825 | 0.943 | 3.36481 | 3.42161 | 1.017 | 0.190 | 0.093 | 0.489 | 0 | yes |
| ch_intrinsics | 68 / 0 / 1 | 12 | 825 | 795 | 0.964 | 2.18081 | 2.38504 | 1.094 | 0.060 | 0.028 | 0.467 | 0 | yes |
| diffeq1 | 32 / 5 / 0 | 16 | 5160 | 5050 | 0.979 | 15.9904 | 15.9997 | 1.001 | 0.240 | 0.087 | 0.362 | 4 | yes |
| diffeq2 | 21 / 7 / 0 | 18 | 5097 | 4733 | 0.929 | 12.3169 | 12.1981 | 0.990 | 0.160 | 0.073 | 0.456 | 4 | yes |
| mcml | 7302 / 27 / 159 | 101 | 718639 | 693521 | 0.965 | 47.0709 | 48.0662 | 1.021 | 327.680 | 233.469 | 0.712 | 471 | yes |
| mkDelayWorker32B | 473 / 0 / 45 | 50 | 15924 | 13599 | 0.854 | 8.181 | 6.7593 | 0.826 | 1.860 | 0.607 | 0.326 | 2 | yes |
| mkPktMerge | 31 / 0 / 15 | 28 | 9125 | 8166 | 0.895 | 4.09928 | 4.24477 | 1.035 | 0.390 | 0.137 | 0.351 | 0 | yes |
| mkSMAdapter4B | 189 / 0 / 5 | 20 | 11771 | 11844 | 1.006 | 4.67723 | 4.694 | 1.004 | 0.790 | 0.544 | 0.689 | 7 | yes |
| or1200 | 258 / 1 / 2 | 27 | 31226 | 31997 | 1.025 | 8.49305 | 9.22165 | 1.086 | 2.590 | 1.350 | 0.521 | 13 | yes |
| raygentop | 123 / 8 / 0 | 19 | 11237 | 10998 | 0.979 | 4.53 | 4.62372 | 1.021 | 0.570 | 0.397 | 0.696 | 1 | yes |
| sha | 155 / 0 / 0 | 17 | 11670 | 11198 | 0.960 | 9.56668 | 9.57774 | 1.001 | 0.400 | 0.566 | 1.415 | 9 | yes |
| spree | 64 / 1 / 3 | 14 | 7318 | 7106 | 0.971 | 10.858 | 11.0821 | 1.021 | 0.270 | 0.165 | 0.611 | 2 | yes |
| stereovision0 | 725 / 0 / 0 | 34 | 40349 | 36812 | 0.912 | 3.44133 | 3.50522 | 1.019 | 3.630 | 2.192 | 0.604 | 0 | yes |
| stereovision1 | 770 / 40 / 0 | 40 | 79581 | 75936 | 0.954 | 5.13017 | 5.12796 | 1.000 | 4.160 | 3.950 | 0.950 | 0 | yes |
| stereovision2 | 1977 / 324 / 0 | 104 | 311596 | 319145 | 1.024 | 14.6044 | 13.2032 | 0.904 | 30.010 | 7.643 | 0.255 | 312 | yes |
| stereovision3 | 14 / 0 / 0 | 7 | 456 | 450 | 0.987 | 2.65898 | 2.67608 | 1.006 | 0.010 | 0.011 | 1.100 | 0 | yes |
| Geomean |  |  |  |  | 0.967 |  |  | 1.000 |  |  | 0.609 |  |  |

Routed by VPR's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 124 | 126 | 1.016 | 162 | 76.2861 | 75.6907 | 0.992 | 1310007 | 1310679 | 1.001 | yes |
| LU8PEEng | 92 | 96 | 1.043 | 120 | 76.1097 | 77.0562 | 1.012 | 325298 | 323342 | 0.994 | yes |
| arm_core | 104 | 110 | 1.058 | 136 | 18.6216 | 18.8139 | 1.010 | 184148 | 186675 | 1.014 | yes |
| bgm | 70 | 68 | 0.971 | 92 | 20.4561 | 19.6491 | 0.961 | 366218 | 349890 | 0.955 | yes |
| blob_merge | 58 | 66 | 1.138 | 76 | 15.2323 | 15.5895 | 1.023 | 66256 | 70073 | 1.058 | yes |
| boundtop | 42 | 40 | 0.952 | 54 | 3.55486 | 3.4719 | 0.977 | 3124 | 2967 | 0.950 | yes |
| ch_intrinsics | 42 | 38 | 0.905 | 54 | 2.49575 | 2.49995 | 1.002 | 1416 | 1310 | 0.925 | yes |
| diffeq1 | 60 | 56 | 0.933 | 78 | 17.0822 | 17.2566 | 1.010 | 8635 | 8721 | 1.010 | yes |
| diffeq2 | 46 | 46 | 1.000 | 60 | 12.9774 | 12.9064 | 0.995 | 9780 | 9245 | 0.945 | yes |
| mcml | 124 | 138 | 1.113 | 162 | 46.3566 | 45.3936 | 0.979 | 921573 | 897314 | 0.974 | yes |
| mkDelayWorker32B | 38 | 36 | 0.947 | 50 | 8.43017 | 6.86275 | 0.814 | 22576 | 20033 | 0.887 | yes |
| mkPktMerge | 46 | 38 | 0.826 | 60 | 4.26319 | 4.40979 | 1.034 | 13566 | 12801 | 0.944 | yes |
| mkSMAdapter4B | 50 | 58 | 1.160 | 64 | 4.82225 | 4.83662 | 1.003 | 17892 | 18040 | 1.008 | yes |
| or1200 | 92 | 96 | 1.043 | 120 | 8.64458 | 8.73672 | 1.011 | 43336 | 44592 | 1.029 | yes |
| raygentop | 58 | 58 | 1.000 | 76 | 4.74311 | 4.75768 | 1.003 | 18983 | 18459 | 0.972 | yes |
| sha | 68 | 64 | 0.941 | 88 | 10.0346 | 9.74162 | 0.971 | 15449 | 15128 | 0.979 | yes |
| spree | 62 | 62 | 1.000 | 80 | 11.2813 | 11.2002 | 0.993 | 12039 | 11903 | 0.989 | yes |
| stereovision0 | 48 | 46 | 0.958 | 62 | 3.60577 | 3.69935 | 1.026 | 55531 | 51045 | 0.919 | yes |
| stereovision1 | 80 | 76 | 0.950 | 104 | 5.57953 | 5.30515 | 0.951 | 114217 | 110512 | 0.968 | yes |
| stereovision2 | 68 | 64 | 0.941 | 88 | 15.8253 | 13.8538 | 0.875 | 422434 | 429884 | 1.018 | yes |
| stereovision3 | 30 | 36 | 1.200 | 40 | 2.65817 | 2.57512 | 0.969 | 783 | 714 | 0.912 | yes |
| Geomean |  |  | 1.001 |  |  |  | 0.980 |  |  | 0.973 |  |

Routed by guide's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 122 | 128 | 1.049 | 162 | 76.128 | 75.4897 | 0.992 | 1297548 | 1299899 | 1.002 | yes |
| LU8PEEng | 88 | 100 | 1.136 | 120 | 75.6488 | 76.9099 | 1.017 | 322552 | 321797 | 0.998 | yes |
| arm_core | 102 | 110 | 1.078 | 136 | 18.6017 | 18.8028 | 1.011 | 183390 | 185067 | 1.009 | yes |
| bgm | 70 | 68 | 0.971 | 92 | 20.4705 | 19.6048 | 0.958 | 365252 | 348800 | 0.955 | yes |
| blob_merge | 62 | 66 | 1.065 | 76 | 15.1616 | 15.4486 | 1.019 | 65834 | 69694 | 1.059 | yes |
| boundtop | 42 | 40 | 0.952 | 54 | 3.55154 | 3.47301 | 0.978 | 3150 | 2951 | 0.937 | yes |
| ch_intrinsics | 42 | 38 | 0.905 | 54 | 2.52005 | 2.50282 | 0.993 | 1485 | 1337 | 0.900 | yes |
| diffeq1 | 60 | 52 | 0.867 | 78 | 17.6167 | 17.258 | 0.980 | 8726 | 8598 | 0.985 | yes |
| diffeq2 | 50 | 46 | 0.920 | 60 | 13.3234 | 12.9674 | 0.973 | 9799 | 9211 | 0.940 | yes |
| mcml | 126 | 140 | 1.111 | 162 | 46.4519 | 45.2693 | 0.975 | 918230 | 892479 | 0.972 | yes |
| mkDelayWorker32B | 40 | 36 | 0.900 | 50 | 8.46292 | 6.85168 | 0.810 | 22086 | 19668 | 0.891 | yes |
| mkPktMerge | 46 | 40 | 0.870 | 60 | 4.26762 | 4.41975 | 1.036 | 13395 | 12589 | 0.940 | yes |
| mkSMAdapter4B | 50 | 56 | 1.120 | 64 | 4.80151 | 5.05617 | 1.053 | 17939 | 18629 | 1.038 | yes |
| or1200 | 92 | 112 | 1.217 | 120 | 8.63351 | 8.72343 | 1.010 | 43678 | 44588 | 1.021 | yes |
| raygentop | 56 | 58 | 1.036 | 76 | 4.86012 | 4.93969 | 1.016 | 18933 | 18335 | 0.968 | yes |
| sha | 72 | 64 | 0.889 | 88 | 10.1613 | 10.3606 | 1.020 | 15463 | 15026 | 0.972 | yes |
| spree | 62 | 64 | 1.032 | 80 | 11.2769 | 11.1869 | 0.992 | 11963 | 11991 | 1.002 | yes |
| stereovision0 | 50 | 46 | 0.920 | 62 | 3.60245 | 3.69492 | 1.026 | 55592 | 51059 | 0.918 | yes |
| stereovision1 | 82 | 72 | 0.878 | 104 | 5.4429 | 5.16188 | 0.948 | 114462 | 110730 | 0.967 | yes |
| stereovision2 | 72 | 68 | 0.944 | 88 | 15.7821 | 13.8505 | 0.878 | 420551 | 428049 | 1.018 | yes |
| stereovision3 | 30 | 36 | 1.200 | 40 | 2.65817 | 2.57512 | 0.969 | 777 | 700 | 0.901 | yes |
| Geomean |  |  | 0.997 |  |  |  | 0.982 |  |  | 0.970 |  |

### What the results say

- **After routing, `guide`'s placements are as good as VPR's or better, which is M8's exit for
  FPGAs.** With VPR's router, critical paths are 0.2% shorter on MCNC and 2.0% shorter on the
  Verilog suite, with 4.5% and 2.7% less wire, and channels 1.3% narrower and as wide. With
  `guide`'s router, 0.6% and 1.8% shorter, 4.7% and 3.0% less wire, and 1.9% and 0.3%
  narrower.
- **Before routing**, by VPR's own measures, `guide`'s placements need 4.5% less wire on MCNC
  and 3.3% less on the Verilog suite, and VPR's delay model puts their critical paths 4.1%
  longer on MCNC and the same on the Verilog suite. Routing undoes that MCNC difference: see
  [the delay model](#what-it-took-1) below.
- **Channel width is where `guide` does worst.** It is the same on average, but the largest
  Verilog circuits need wider channels on `guide`'s placements: mcml 11% (138 tracks against
  124 with VPR's router), arm_core 6%, LU8PEEng 4% and LU32PEEng 2%, as do blob_merge (14%),
  mkSMAdapter4B (16%) and the tiny stereovision3 (36 against 30). Nothing in the placer's cost
  looks at congestion, as VPR's does not either; its placements are denser, with less wire and
  more of it in some channels.
- **The largest gains are on timing**: stereovision2's critical path is 12.5% shorter (13.85 ns
  against 15.83 ns) and mkDelayWorker32B's 19% shorter, with less wire on both. The worst are
  dsip, 8% longer, and seq, 5% longer, both with less wire.
- **Time**: 0.61 times VPR's on the Verilog suite, 232 s against 341 s on LU32PEEng, and 1.57
  times on MCNC, where both take fractions of a second and the global placement's fixed cost
  counts.

### How the default came about

On the 39 circuits but LU32PEEng and mcml, which take hours of routing, two other settings went
through the same comparison. Each row has the geometric means of the ratios to VPR's placement,
with VPR's measures of the placement and VPR's router:

| Placer | Suite | BB / VPR | Est. CPD / VPR | Time / VPR | Min W / VPR | CPD / VPR | Wire / VPR |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **Analytic, then annealing, `inner_num` 1** (the default) | MCNC | **0.955** | **1.041** | **1.57** | **0.987** | **0.998** | **0.955** |
| Annealing from random, `inner_num` 0.5 | MCNC | 0.985 | 1.035 | 1.58 | 0.994 | 0.995 | 0.980 |
| Default, without timing | MCNC | 0.913 | 1.106 | 1.15 | 0.960 | 1.062 | 0.919 |
| **Analytic, then annealing, `inner_num` 1** (the default) | Verilog, 19 | **0.965** | **0.999** | **0.60** | **0.994** | **0.980** | **0.971** |
| Annealing from random, `inner_num` 0.5 | Verilog, 19 | 1.004 | 1.036 | 0.44 | 1.028 | 1.021 | 0.998 |
| Default, without timing | Verilog, 19 | 0.929 | 1.068 | 0.42 | 0.976 | 1.043 | 0.947 |

- **Annealing from a random placement**, hot, at VPR's `inner_num` of 0.5, as VPR placed
  before version 9, uses 2% less wire than VPR's placements on MCNC and as much on the Verilog
  suite, with critical paths 0.5% shorter and 2% longer. Global placement first takes 2.5% to
  2.7% more wire off, and 4% off the Verilog suite's critical paths, while MCNC's stay within
  0.3%, in the same time on MCNC and 1.4 times as long on the Verilog suite. On stereovision2 a random start came to 17% more wire before routing
  than VPR's placement at this effort, and 8% more at four times it; VPR 9 starts cold from a
  constructive placement too, at the standard deviation of moves over 64.
- **Without timing**, for wirelength alone, placements use 5% to 8% less wire than VPR's after
  routing, and give 3% to 6% longer critical paths. With VPR's tradeoff of 0.5, `guide` gives
  up some of that wire for delay, as VPR does.
- **Effort**, measured on ten circuits before the full runs, by VPR's measures before routing:
  `inner_num` 0.5 came to 0.997 of VPR's wire in two fifths of its time, 1 to 0.991 in two
  thirds, and 2 to 0.981 in 1.2 times. A temperature of 0.02 or 0.2 of the standard deviation
  at the start, instead of 0.05, or a first range limit of 4 or 16 steps, instead of 8, moved
  wire and delay by under 1.5%.

### What it took

- **VPR's own numbers.** To measure placements as VPR does, `bb_estimate` had to equal what
  VPR prints, and three things stood in the way, found against a VPR built to print each net:
  VPR 9 no longer clips a net's box to the device less its outer columns and rows, as earlier
  versions did; it puts a pin of a block over several tiles on its own tile, which the class's
  `SOURCE` node does not say, as VPR writes that node over the whole block, so pins are where
  their `IPIN` or `OPIN` nodes are; and it leaves out constant nets as well as clocks.
- **A cold start from a good placement.** Annealing from a random placement, which VPR did
  before version 9, came out 17% longer than VPR's placement on stereovision2, whose grid is
  sized for its 324 multipliers, and annealing hot from the global placement, as VPR 9's
  schedule does from its own, undid most of it. Annealing at a twentieth of the moves'
  standard deviation over 8 steps from the legalized global placement, the detailed placement of
  M7, came to VPR's wirelength there.
- **A delay model.** VPR's estimate of `guide`'s placements is 4% longer than of its own on
  MCNC and about the same on the Verilog suite, though they route to critical paths as short or
  shorter. The two
  models differ: `PlaceDelayModel` is the least delay that Dijkstra finds on the graph at the
  relaxed width, with the graph's own step delays, and VPR's routes with its router on a graph
  of a width of its choosing. `guide`'s model puts the critical paths of VPR's own placements
  at 0.87 to 1.02 of VPR's estimate, 0.93 in the middle.

### What this does not show

- One seed per circuit for each placer, and one packing. Single circuits move by several
  percent between settings, as the per-circuit tables show, which is more than the means do.
- Placement times are of runs a few days apart on a shared machine, VPR's from the routing
  comparisons; on MCNC they are fractions of a second, where fixed costs count.
- Clock regions: VTR's architectures have none, so nothing here needs them. Titan23 and the
  ISPD contest sets were not run, and neither was nextpnr.
- `guide` reads the device from VPR's graph rather than from the architecture's layout, and
  follows only blocks whose type has the name of a tile type, which is how VTR's architectures
  have it.

## PCB clearance checks against KiCad

This is the comparison that M4's exit criterion asks for: `geom::drc::check`, on the
[curated boards](../testdata/kicad/README.md) as `kicad::read` reads them, against
`kicad-cli pcb drc` from KiCad 10.0.3, finding by finding.

### How

Eleven real boards, of two to six layers, from 38 items to over 3000. Each is checked
under its own rules, and again with every net class's clearance raised by up to 0.5 mm, which
leaves the copper alone and turns it into many more findings: fine-pitch pins, zone fills
around pads, arcs next to vias. The raise stops where KiCad would report 499 clearance
violations, as KiCad then stops looking (`EXTENDED_ERROR_LIMIT` in its `drc_engine.cpp`), and
which ones it has found by then depends on its threads. Its other checks stop at 199.

A finding is a kind (a clearance violation, a short, a hole clearance violation or holes too
close) and the UUIDs of the two items. Ours are made with
[`KICAD_CLEARANCE_SLACK`](../src/geom/drc.rs) and an arc tolerance of 10 nm. KiCad runs with
`--severity-all --all-track-errors`, so that it reports every violation of every track, and the
violations that the projects exclude.

```text
cargo test --features kicad curated_boards_match              # against KiCad's recorded results
GUIDE_KICAD=1 cargo test --release --features kicad curated   # against kicad-cli, live, 30 s
```

The second also checks that KiCad's results are still those recorded, re-checks the arcs
described below, and reads each board again after `kicad-cli pcb upgrade` has saved it in
KiCad 10's format, which must give the same board.

### Results

| Board | Rules | KiCad's findings | Ours | Both | Explained differences | Unexplained |
| --- | --- | --- | --- | --- | --- | --- |
| GB-BRK-LINK-A | own | 0 | 0 | 0 | - | 0 |
| GB-BRK-LINK-A | +0.1 mm | 3 | 3 | 3 | - | 0 |
| GB-BRK-LINK-A | +0.3 mm | 19 | 19 | 19 | - | 0 |
| GB-BRK-LINK-A | +0.5 mm | 30 | 30 | 30 | - | 0 |
| GB-LIVE32 | own | 0 | 0 | 0 | - | 0 |
| GB-LIVE32 | +0.05 mm | 339 | 339 | 339 | - | 0 |
| TS4231-BOB | own | 0 | 0 | 0 | - | 0 |
| TS4231-BOB | +0.1 mm | 12 | 12 | 12 | - | 0 |
| TS4231-BOB | +0.3 mm | 78 | 78 | 78 | - | 0 |
| TS4231-BOB | +0.5 mm | 141 | 141 | 141 | - | 0 |
| lisn | own | 0 | 0 | 0 | - | 0 |
| lisn | +0.1 mm | 43 | 43 | 43 | - | 0 |
| lisn | +0.3 mm | 256 | 261 | 256 | 5 zone sweep | 0 |
| lisn | +0.5 mm | 357 | 362 | 357 | 5 zone sweep | 0 |
| placebo | own | 2 | 2 | 2 | - | 0 |
| placebo | +0.1 mm | 122 | 123 | 122 | 1 arc | 0 |
| placebo | +0.2 mm | 272 | 274 | 272 | 2 arc | 0 |
| flea | own | 199 | 199 | 199 | - | 0 |
| flea | +0.05 mm | 477 | 477 | 477 | - | 0 |
| nk3-nrf-mini | own | 1 | 5 | 1 | 4 pad holes | 0 |
| nk3-nrf-mini | +0.05 mm | 418 | 420 | 416 | 4 pad holes, 2 text | 0 |
| udb-s | own | 4 | 4 | 4 | - | 0 |
| udb-s | +0.1 mm | 281 | 281 | 281 | - | 0 |
| udb-s | +0.3 mm | 392 | 392 | 392 | - | 0 |
| isi-channel | own | 0 | 0 | 0 | - | 0 |
| isi-channel | +0.5 mm | 4 | 4 | 4 | - | 0 |
| lpddr4-testbed | own | 0 | 0 | 0 | - | 0 |
| lpddr4-testbed | +0.02 mm | 274 | 274 | 274 | - | 0 |
| lpddr4-testbed | +0.05 mm | 403 | 404 | 403 | 1 arc | 0 |
| ddr5-testbed | own | 0 | 0 | 0 | - | 0 |
| ddr5-testbed | +0.01 mm | 233 | 233 | 233 | - | 0 |

In all, 31 checks: KiCad made 4360 findings and we made 4380, of which 4358 are the same. Of
the 24 differences, 2 are KiCad's only and 22 ours only, and each is explained below, from
KiCad's source at tag `10.0.3`. None is unexplained. The boards with findings under their own rules are as
their designers left them: about half of `flea`'s 199 are between pads of no net in one
footprint, `U1`, which KiCad checks against each other.

### The differences

**Zone sweep** (10, on `lisn`: ours only). KiCad's `testZonesToZones` compares the fills of two
zones only if their bounding boxes meet, with no margin for the clearance, so it never compares
fills side by side across a gap. Where it does compare them, it sorts their edges from left to
right and stops measuring at the first edge that starts beyond the end of the one it measures
from, however close that is. The test emulates this code, and it finds none of the ten, which
are five pairs of fills about 0.5 mm apart, under clearances of 0.7 mm and then 0.9 mm.

**Arc** (4, three on `placebo` and one on `lpddr4-testbed`: ours only). KiCad's `SHAPE_ARC::NearestPoints`, in `shape_arc.cpp`,
gets two cases wrong. Against a circle, such as a via or a round pad, it considers each end of
the arc only if `sliceContainsPoint` accepts it, which tests the end's angle in floating point
and can refuse the arc's own end. Against another arc, the loops that measure each arc's ends
against the other arc assign the running minimum without comparing with it, so a later, farther
candidate replaces a nearer one. The three are an arc and a round pad 0.2753 mm apart, under
two sets of rules, and two arcs 0.3856 mm apart, under a clearance of 0.4 mm; on
`lpddr4-testbed`, two arcs of its meanders. For the first,
KiCad measures 0.4723 mm, which is the distance from the arc's start, where its end is nearer;
with the same arc written the other way round, KiCad measures 0.2753 mm. Each of the three is
checked again by KiCad itself, with those arcs split into segments within 5 nm of them, and
KiCad then reports it.

The two DRAM boards, added for M6, agreed with KiCad as they came: they needed nothing new.

**Pad holes** (8, on `nk3-nrf-mini`: ours only). KiCad's `testPadClearances` checks each pair of
pads once per layer, from whichever of the two its threads reach first, and `testPadAgainstItem`
then checks only the other pad's hole against this one's copper, and only where this one has
copper on the layer. So a pad's hole can go unchecked against another pad, depending on the
order. These are two non-plated mounting holes, 0.11 mm from the edge of the custom touch pad on
each side of the board, under a hole clearance of 0.15 mm. KiCad does report a third mounting
hole of the board against a pad, test pad `TP1`.

**Text** (2, on `nk3-nrf-mini`: KiCad's only). Text on copper, here layer numbers on the inner
layers, too close to a zone. `kicad::read` doesn't read text, which needs KiCad's fonts, and
says so in its warnings.

### What agreeing took

Most of the work was in finding what KiCad checks, rather than in geometry. In the order they
were found:

- KiCad stops at 499 clearance violations, as above, which first looked like it missed pads.
- Two nets of a differential pair, such as `D+` and `D-`, may come as close as their class's gap,
  when that is less than its clearance; a larger gap changes nothing. This is
  [`RuleSpec::pairs`](../src/geom/rules.rs).
- A zone's own clearance is what it was filled with, and not a rule. Pads' and footprints' own
  clearances are, and replace the classes' (`drc::clearance`).
- Copper of no net is two things to KiCad: tracks, vias and graphics of no net are one net,
  never checked against each other, but pads and zones of no net are checked against each
  other. And KiCad calls copper touching a short only where it can name two nets.
- KiCad gives tracks the net of the pads they touch, whatever the file says, as it loads a
  board, which `kicad::read` does too.
- KiCad doesn't report a hole as too close to copper that the hole's own pad is already reported
  against.
- Arcs: our own arc tolerance of 100 nm made two findings within KiCad's slack; at 10 nm there
  are none.

### What this does not show

- The boards were mostly clean, as released boards are, so most findings come from raised
  clearances. That tests the geometry and the rules, but not every way a real board goes wrong:
  none of them has a short between two nets, so no short is compared with KiCad's. (The random
  boards of `geom`'s own KiCad test avoid shorts too, as KiCad gives copper with no pad the
  net of what it touches, which hides them.)
- Both checks read the same files, through different code: an item we read wrong in the same
  way KiCad does would not show here. Reading each board again in KiCad 10's format, as the
  live test does, catches some of that.
- Custom rules (`.kicad_dru` files) are not read, and none of these boards has one. Neither are
  padstacks with other shapes on other layers, text, Bézier curves, nor pads and vias without
  copper where nothing connects to them; `kicad::read` warns of each.
- Copper edge clearance, solder mask and silkscreen checks are KiCad's, and not compared.

## PCB: DSN designs against KiCad boards

`dsn::read` reads the Specctra designs that KiCad's Freerouting workflow exports, into the same
`drc::Board` and `Rules` as `kicad::read`. This compares the two readings of each
[curated board](../testdata/kicad/README.md): its `.kicad_pcb` and project, and the `.dsn` that
KiCad 10.0.3's own exporter (`pcbnew.ExportSpecctraDSN`) wrote from it, under the same scenarios
as above, finding by finding.

### How

Each DSN item is matched to the KiCad item it came from: tracks by their layer and ends, vias by
their position, pads by their component, number and the box of their copper, all within 3 µm.
Every DSN track, via and pad finds its KiCad item, and every KiCad track, via and pad with copper
is in the DSN. Both readings are then checked with KiCad's slack and an arc tolerance of 10 nm,
the DSN's rules raised as the project's are, and their findings compared by the UUIDs of the
KiCad items. A short and a clearance violation are one kind here, as which a pair is can turn on
rounding. Each difference must be explained by something a DSN file doesn't carry, or by
rounding whose size is checked: the two readings' gaps for the pair must straddle what decides,
the clearance less KiCad's slack, and be within 1.5 µm of each other (6.5 µm for a rounded pad,
which KiCad writes as a polygon), or within an arc's sagitta of each other for an arc.

```text
cargo test --release --all-features curated_dsn
```

### Results

| Board | Rules | KiCad reading | DSN reading | Both | KiCad reading only | DSN reading only |
| --- | --- | --- | --- | --- | --- | --- |
| GB-BRK-LINK-A | own | 0 | 0 | 0 | - | - |
| GB-BRK-LINK-A | +0.1 mm | 3 | 3 | 3 | - | - |
| GB-BRK-LINK-A | +0.3 mm | 19 | 9 | 9 | 10 zone fills | - |
| GB-BRK-LINK-A | +0.5 mm | 30 | 20 | 20 | 10 zone fills | - |
| GB-LIVE32 | own | 0 | 0 | 0 | - | - |
| GB-LIVE32 | +0.05 mm | 339 | 339 | 339 | - | - |
| TS4231-BOB | own | 0 | 0 | 0 | - | - |
| TS4231-BOB | +0.1 mm | 12 | 12 | 12 | - | - |
| TS4231-BOB | +0.3 mm | 78 | 78 | 78 | - | - |
| TS4231-BOB | +0.5 mm | 141 | 118 | 118 | 23 zone fills | - |
| lisn | own | 0 | 3 | 0 | - | 3 rounding |
| lisn | +0.1 mm | 43 | 47 | 43 | - | 4 rounding |
| lisn | +0.3 mm | 261 | 147 | 147 | 114 zone fills | - |
| lisn | +0.5 mm | 362 | 221 | 217 | 145 zone fills | 4 rounding |
| placebo | own | 2 | 9 | 2 | - | 4 arcs, 3 rounding |
| placebo | +0.1 mm | 123 | 81 | 77 | 46 zone fills | 2 arcs, 2 rounding |
| placebo | +0.2 mm | 274 | 209 | 208 | 9 arcs, 57 zone fills | 1 arc |
| flea | own | 199 | 323 | 177 | 22 holes | 56 arcs, 54 custom pads, 10 holes as keepouts, 26 rounding |
| flea | +0.05 mm | 477 | 534 | 453 | 2 arcs, 22 holes | 16 arcs, 55 custom pads, 10 holes as keepouts |
| nk3-nrf-mini | own | 5 | 45 | 0 | 5 holes | 37 custom pads, 6 holes as keepouts, 1 pad's own clearance, 1 rounding |
| nk3-nrf-mini | +0.05 mm | 420 | 327 | 280 | 5 holes, 135 zone fills | 40 custom pads, 6 holes as keepouts, 1 pad's own clearance |
| udb-s | own | 4 | 93 | 4 | - | 4 arcs, 3 rounding, 82 routing widths |
| udb-s | +0.1 mm | 281 | 251 | 68 | 213 zone fills | 1 arc, 100 pairs' gaps, 82 routing widths |
| udb-s | +0.3 mm | 392 | 390 | 158 | 1 arc, 233 zone fills | 1 arc, 149 pairs' gaps, 82 routing widths |
| isi-channel | own | 0 | 0 | 0 | - | - |
| isi-channel | +0.5 mm | 4 | 0 | 0 | 4 zone fills | - |
| lpddr4-testbed | own | 0 | 0 | 0 | - | - |
| lpddr4-testbed | +0.02 mm | 274 | 171 | 171 | 13 arcs, 90 zone fills | - |
| lpddr4-testbed | +0.05 mm | 404 | 306 | 303 | 9 arcs, 92 zone fills | 3 rounding |
| ddr5-testbed | own | 0 | 4182 | 0 | - | 4182 routing widths |
| ddr5-testbed | +0.01 mm | 233 | 4415 | 233 | - | 4182 routing widths |

In all, the KiCad reading makes 4380 findings, as in the comparison with KiCad above, and the
DSN reading 12333, of which 3120 are the same. All 10473 differences are explained, and none is
unexplained. Of the 1260 on the KiCad reading only, 1172 are zone fills, 54 holes and 34 arcs.
Of the 9213 on the DSN reading only, 8610 are routing widths, 249 pairs' gaps, 186 custom pads,
85 arcs, 49 rounding, 32 holes as keepouts and 2 pads' own clearances. `ddr5-testbed`'s 4182
routing widths are all its tracks of 0.1016 mm, under the 0.12 mm of its only net class, which
the DSN gives as the width of every net's wire. Two arcs side by side may each be as far as its
sagitta from its chord, so the gap between their chords may differ by both sagittas, which four
findings of `lpddr4-testbed`'s meanders needed.

### The differences

All but rounding are things a design doesn't carry, as `dsn`'s documentation lists them:

- **Zone fills** (KiCad only). A design has planes, which are outlines to pour, and not the
  fills; KiCad exports teardrops as planes too. They are read, and not checked as copper.
- **Holes** (KiCad only), and **holes as keepouts** (DSN only). Pins have no drills in a design,
  so there are no hole clearances or hole-to-hole checks. KiCad exports non-plated holes as
  keepouts, grown by the board's hole clearance, which tracks and vias then enter.
- **Arcs.** KiCad exports an arc track as one straight wire from end to end. Each of these is a
  pair whose two gaps differ by no more than the arc's sagitta, and the rounding.
- **Custom pads** (DSN only). KiCad exports a custom pad as the convex hull of its shape, which
  covers what the pad doesn't: on `nk3-nrf-mini`, the curved touch pad, and on `flea`, the pads
  of `U1` next to its pads of no number.
- **Pairs' gaps** (DSN only). The two nets of a differential pair may come as close as their
  class's gap, which a design doesn't say, so the class's clearance applies between them.
- **Pads' own clearances** (DSN only). A design has no clearance of a pad's own.
- **Routing widths** (DSN only). A design's widths are what to route at, and become the rules'
  widths, where the KiCad reading has KiCad's minimum width. `udb-s` has 82 tracks narrower than
  their class's width, and `ddr5-testbed` 4182.
- **Rounding.** KiCad writes coordinates to six significant digits in micrometres: 0.1 µm below
  100 mm, and 1 µm beyond. Each of these is a pair at the very edge of the clearance, whose two
  gaps are within 1.5 µm of each other, or 6.5 µm for a rounded pad.

### Sessions into KiCad

The other direction: `dsn::write_session` writes a session from each curated board's own tracks
and vias, as `kicad::read` reads them, and KiCad's own `pcbnew.ImportSpecctraSES` imports it into
the board, replacing its tracks and vias.

```text
GUIDE_KICAD=1 cargo test --release --all-features curated_sessions
```

On all eleven boards, every straight track and every via comes back exactly, to the nanometre,
with its width, layers, drill and net. A session has no arcs that KiCad reads (`qarc` is
refused), so the 1192 arcs of five boards come back as polylines within 100 nm of them, 34 159
tracks in all. KiCad's DRC then finds the same as before:

- on the six boards with no arcs, as many violations of each kind, and as many unconnected
  items;
- on every board, the same pairs of items, where an arc and the tracks it became are one item,
  with the same smallest gaps to within 0.3 µm. An arc too close to something is now several
  tracks too close to it, so there are more violations. On `flea` that takes KiCad's clearance
  violations past its limit of 499, from 177 to 503, so its clearances and hole clearances can't
  be compared there. Solder mask bridges are left out of this, as KiCad reports one pair of
  the items a mask opening bridges, whichever its threads reach first, and that changes from
  run to run on the same board.

The same test with `curated_dsn_files` checks that KiCad's exporter still writes the DSN files in
`testdata/kicad`, and `GUIDE_KICAD_UPDATE=1` writes them again.

### Freerouting

For the comparison with Freerouting that M5's exit asks for, Freerouting 2.5.0 routes these
designs headless. It isn't installed: its jar needs Java 25, and its native `freerouting-cli`
build for Linux crashes as it starts routing (a GraalVM serialisation error), so it runs from
the jar on a JRE unpacked anywhere, such as Adoptium's:

```text
jre/bin/java -Djava.awt.headless=true -jar freerouting-2.5.0.jar \
    -de board.dsn -do board.ses --router.autorouter.max_passes=20 -mt 4 --gui.enabled=false
```

On `TS4231-BOB` it keeps the existing wiring, routes the two nets left, and writes a session in
the same form as `write_session`'s, which KiCad imports. It reports one clearance violation that
neither KiCad nor either reading here finds, between a via of `GND` and pad 2 of `R1`, which it
calls fixed and leaves alone: that is Freerouting's own approximation of a via, [explained
below](#the-clearance-violation-only-freerouting-finds). The [comparison of
routers](#pcb-routing-against-freerouting) is below too. Nothing here depends on Freerouting.

### What this does not show

- Both readings are ours. That the DSN reading agrees with the KiCad reading, and the KiCad
  reading with KiCad's DRC, says the DSN reader is right about what KiCad exports; it says
  nothing about designs from other tools, which can use what KiCad's exporter doesn't: other
  units, `qarc` wires, padstacks with other shapes on other layers, layer rules. Those are
  tested on small hand-written designs only.
- KiCad's exporter refuses two footprints of one reference, so two of `nk3-nrf-mini`'s three
  mounting holes `REF**` were renamed for it, `REF**_2` and `REF**_3`, which changes no copper.

## PCB routing of the curated boards

`pcb::route` on the nine [curated boards](../testdata/kicad/README.md), each stripped of its
tracks, arcs, vias and zone fills, so that every net with pads on two copper islands has to be
routed again, ground and power included, which the originals mostly connect through zones.
These are the results of the router as it was before zones, which it still gives with
`Options::planes` off; [below](#pcb-zones-and-planes), the same boards keep their zones'
outlines, and their nets connect through them. Copper graphics, pads, keepouts, the board's edge, text on copper and solder mask openings
stay. Each net gets its class's track width and via size.

### How

```text
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_KICAD=1 cargo test --release --features kicad curated_boards_route -- --nocapture
```

The first routes the boards and checks each result with `drc::check`, which must find no
violation involving a routed track or via, and `pcb::unconnected`, which must find every routed
net connected. The second writes each routed board with `kicad::replace_routing` and runs
`kicad-cli pcb drc` 10.0.3 on it, at error severity: it must find no violation involving a
routed item, and on a board with every net routed, no missing connection. The three smallest
boards route in every `cargo test` too. One thread, on an AMD Ryzen Threadripper PRO 9995WX,
with the default options.

### Results

| Board | Nets | Connections | DRC violations | Vias: routed → after cleanup (original) | Track, mm: routed → after cleanup (original) | Seconds | KiCad |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 6/6 | 8/8 | 0 | 0 → 0 (0) | 128 → 128 (115) | 0.01 | 0 violations, 0 unconnected |
| `TS4231-BOB` | 8/8 | 13/13 | 0 | 2 → 1 (10) | 41 → 43 (39) | 0.01 | 0 violations, 0 unconnected |
| `lisn` | 18/18 | 60/60 | 0 | 15 → 10 (0) | 312 → 329 (328) | 0.08 | 0 violations, 0 unconnected |
| `isi-channel` | 7/7 | 29/29 | 0 | 2 → 0 (0) | 780 → 771 (1319) | 0.21 | 0 violations, 0 unconnected |
| `placebo` | 23/23 | 50/50 | 0 | 19 → 9 (10) | 306 → 321 (286) | 0.13 | 0 violations, 0 unconnected |
| `udb-s` | 8/8 | 33/33 | 0 | 15 → 12 (24) | 180 → 187 (172) | 0.95 | 0 violations, 0 unconnected |
| `flea` | 24/24 | 79/79 | 0 | 56 → 52 (31) | 1602 → 1607 (1704) | 5.43 | 0 violations, 0 unconnected |
| `GB-LIVE32` | 79/79 | 197/197 | 0 | 186 → 142 (140) | 2654 → 2705 (2315) | 33.9 | 0 violations, 0 unconnected |
| `nk3-nrf-mini` | 29/30 | 82/83 | 0 | 62 → 60 (60) | 311 → 307 (349) | 24.0 | 0 violations, 1 unconnected |

A connection is a terminal to join to its net's tree: a net of `n` islands of copper, pads that
touch counted as one, needs `n - 1`. The original track length counts arcs as two chords, and
leaves out what zones connect, so it is lower than it would be for the same connections.
`isi-channel`'s original tracks are long because they are length-matched test channels, which
this router does not attempt.

Eight boards are complete, and KiCad finds nothing wrong with any of the nine. `nk3-nrf-mini`
has all its nets but one, ground included, around the nRF52 in its 0.5 mm-pitch aQFN73, where
with 0.12 mm tracks and 0.11 mm clearances nothing passes between pads. Three things got it
there from 28 nets and 56 connections:

- **Escape routing.** Pads that nothing passes between form clusters, and a pad of a cluster
  that can't get out of it on its own layers gets a via first, placed on a grid of 7 µm so as to
  leave room for the others; one whose only way out is long, and that no via fits by, gets that
  way. On `nk3-nrf-mini`, the aQFN's inner pad of `Net-(C6-Pad2)` can only get out on `F.Cu`,
  round a corner of the package, and no via fits by it, as a 5 V USB contact covers `B.Cu`
  there; it gets that way before anything else is routed. The other inner pads there get out
  directly, through the gaps in the outer row, and on `lisn` one pad shut in on all sides gets a
  via in it.
- **Pads with no room at their centre.** Text on copper, `1` beside a ground capacitor, is drawn
  so close to the pad that the box the reader takes for it leaves no room for a track to end at
  the pad's centre. A pad like that is now reached at the nearest point in it that has room.
- **Vias in pads from below.** In the second pass, a route may go to the centre of a pad it has
  yet to reach on a layer the pad isn't on, for a via in it. A ground pad that other nets'
  tracks surround on `F.Cu` is reached so, from `In1.Cu`.

The net left, `/LED_R`, can't be routed without a violation that KiCad reports. Its pad on the
LED `D1` lies wholly inside an opening drawn on `F.Mask`, which also exposes a hole of a
footprint and touches the pad of another net, so KiCad already reports it as a solder mask
bridge with nothing routed. KiCad reports any copper of `/LED_R` in the opening too: a track
leaving the pad in any direction, and a via in the pad, as written and checked by hand, and the
original board's own via there, which KiCad's DRC of the original board reports as well.
The router keeps every net out of an opening of several nets, so it leaves `/LED_R` unrouted.

`udb-s`'s USB pair is routed side by side, which takes it 6 vias and 16 mm more than as two
nets, as [below](#pcb-differential-pairs-and-lengths); its other nets, and every other board,
are as they were before pairs.

Cleanup removed 77 of 357 vias over the nine boards, 22%, and made the track 1% longer: rerouting
a net for fewer vias may lengthen it by up to 5 mm for each via saved, and pulling runs tight
gives back less than that. Neither ever made a violation: they are checked as routing is, and
the results above are after both.

Against the results before escape routing, at commit `8871703`: `nk3-nrf-mini` has one net and
26 connections more, with 60 vias in place of 45 and 307 mm of track in place of 251, for the
nets added; `lisn` has 10 vias in place of 12 and 329 mm in place of 315; `GB-LIVE32` has 142
vias in place of 140 and 2705 mm in place of 2720. Every other board is unchanged.

The router is 3 to 9 times as fast as it was then, with the same results on every board but
those three. In seconds, and in billions of instructions, which a loaded machine changes less:

| Board | Before | After |
| --- | --- | --- |
| `udb-s` | 5.8 s, 72 | 0.66 s, 7 |
| `flea` | 21.3 s, 228 | 4.8 s, 59 |
| `GB-LIVE32` | 98.1 s, 1114 | 34.3 s, 256 |
| `nk3-nrf-mini` | 67.1 s, 879 | 23.8 s, 233 |

- **Indexing arcs by their edges.** An arc of the board's edge was one entry of the index, with
  a box over most of the board, so every step measured to each of its hundreds of edges.
- **Resolving clearances once.** Each query resolved the net classes' clearances through the
  rules again, which cost more than many a query. The world now keeps them by net and layer.
- **Caching step checks.** The world's verdict on a step of one pitch is kept for both of its
  ends while the world is unchanged: across the searches for a net's terminals, and across its
  attempts in its corridor, over the board and with rip-up. So are its verdicts on vias, which a
  search used to forget whenever the net placed one of its own.
- **Giving up early.** Before a search over the whole board, a search of 50,000 nodes at most
  from the terminals not yet reached finds out one that other nets shut in, far sooner than the
  search to it, which has to go through everything outside first. And a net whose search runs
  out of nodes in its corridor is no longer searched for over the board without rip-up, which
  would run out too.

Two more ideas didn't pay. A lower bound that adds a via's cost to reach another layer made
searches take other paths among equals, and lost a net on each hard board; so did weighting the
lower bound of rip-up's searches by 1.3. Routing nets in parallel, on disjoint corridors, would
only speed up the first searches in corridors, which took about a third of the time on
`nk3-nrf-mini` and a tenth on `GB-LIVE32` before the caching: the rest is rip-up, where each
search depends on the one before, so it was not attempted.

### What changes what

Measured at commit `8871703`, before escape routing and the speedups above. Every option of
the table below changes one default. Completion is the boards complete and the
connections made on the two hard boards. Vias, length and seconds are totals over the eight
boards other than `GB-BRK-LINK-A`, which every setting completes in a few milliseconds.

| Options | Boards complete | `GB-LIVE32` | `nk3-nrf-mini` | Vias | Length, mm | Seconds |
| --- | --- | --- | --- | --- | --- | --- |
| Defaults | 8 of 9 | 197/197 | 56/83 | 265 | 6199 | 186 |
| No rip-up | 4 of 9 | 136/197 | 35/83 | 167 | 4845 | 133 |
| No second pass | 7 of 9 | 158/197 | 56/83 | 240 | 5879 | 132 |
| No global routing | 7 of 9 | 158/197 | 55/83 | 224 | 5788 | 182 |
| Corridor margin 0 | 7 of 9 | 191/197 | 40/83 | 298 | 6177 | 256 |
| Corridor margin 2 | 7 of 9 | 158/197 | 40/83 | 224 | 5715 | 257 |
| Layer bias 0.2 | 7 of 9 | 150/197 | 54/83 | 235 | 5717 | 277 |
| No cleanup | 8 of 9 | 197/197 | 56/83 | 343 | 6120 | 167 |

- **Rip-up** is what completes boards: without it, only the four easiest are complete.
- **The second pass**, on a lattice of half the pitch for the nets that the first leaves, is
  what completes `GB-LIVE32`, whose last net is its ground of 40 pads. It costs nothing on boards
  that don't need it.
- **Global routing** completes more than routing every net over the whole board, here
  `GB-LIVE32`, but its corridors make nets detour around congestion that detailed routing
  would have resolved, for a few percent more track on the boards both complete.
- **Steering layers** by global routing, at a 20% premium for a track on a layer that the
  global route didn't take there, loses nets on both hard boards, so it is off.
- **Corridors** of 1 tile's margin beat 0 and 2 on both hard boards.

Results on the two hard boards are chaotic, as for any router that routes nets one at a time:
one changed option moves `nk3-nrf-mini` between 40 and 56 of its 83 connections. The choices
above rest on several boards moving the same way, never on one.

### What this does not show

- Nine boards, all small: the largest has 79 nets. Larger boards would test global routing far
  more than these do.
- Tracks are octilinear and every via goes through the board. No zones are routed here, and
  these results have no lengths to meet: [those below](#pcb-differential-pairs-and-lengths) and
  [those through zones](#pcb-zones-and-planes) do.
- Times are of one thread. Most of the time left on the large boards goes into rip-up's
  searches, and into those that find no path.

## PCB differential pairs and lengths

Differential pairs routed side by side, and nets meandered to their lengths, by `pcb::route`
on the curated boards that have pairs, stripped as [above](#pcb-routing-of-the-curated-boards).
Measured on 2026-11-10 on the same machine, one thread.

### How

```text
GUIDE_ROUTE=1 GUIDE_KICAD=1 cargo test --release --features kicad curated_pairs -- --nocapture
```

`curated_pairs` in [`src/kicad/routing_tests.rs`](../src/kicad/routing_tests.rs) routes each
board three ways and measures each the same way, with `pcb::coupling` and `pcb::net_length`:

- **guide**, with the defaults: pairs side by side, and meanders.
- **apart**, with pairs routed as two nets and no meanders: the router as it was before.
- **original**: the board's own hand routing.

A pair is *coupled* where its positive net's nearest track of the negative net on the same layer
is at the pair's gap, edge to edge, to within 5 µm, measured every 10 µm; arcs are measured as
the arcs they are. The results are checked as all routing is, by `drc::check`, connectivity and
KiCad 10.0.3's DRC, which find nothing wrong on any of the four boards.

The pairs are KiCad's: nets named alike but for a final `P` and `N`, or `+` and `-`, at their
class's `diff_pair_width` and `diff_pair_gap`. The lengths come from custom rules, which
`kicad::length_rules` reads from a `.kicad_dru`. None of these boards has one, so the test gives
them rules for what their designers did by hand:

- `isi-channel` has three channels, each a pair of nets on two coaxial connectors 20 mm apart,
  matched in length within the pair, and of three lengths. Its rules ask for each channel's
  hand-routed length, 55.1, 149.85 and 300 mm, give or take 0.25 mm, and a skew of 0.1 mm at
  most within each pair (`within_diff_pairs`).
- `udb-s`, `GB-LIVE32` and `nk3-nrf-mini` each have a USB pair, which is asked for a skew of
  0.1 mm at most.

### Results

Pairs, after meanders. *Longer* is the longer net's length in millimetres, and *skew* the
difference between the two.

| Board | Pair | Width, gap, mm | guide: coupled | Longer | Skew | Vias | apart: coupled | Longer | Skew | Vias | original: coupled | Longer | Skew | Vias |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `isi-channel` | `/CH1_P`, `/CH1_N` | 0.20, 0.25 | - | 55.10 | 0.0000 | 0 | - | 51.32 | 28.268 | 0 | - | 55.10 | 0.0125 | 0 |
| `isi-channel` | `/CH2_P`, `/CH2_N` | 0.20, 0.25 | - | 149.85 | 0.0000 | 0 | - | 94.88 | 0.1250 | 0 | - | 149.92 | 0.1250 | 0 |
| `isi-channel` | `/CH3_P`, `/CH3_N` | 0.20, 0.25 | - | 300.00 | 0.0000 | 0 | - | 94.75 | 0.0000 | 0 | - | 300.00 | 0.0000 | 0 |
| `udb-s` | `D+`, `D-` | 0.25, 0.20 | 46% | 32.66 | 0.0000 | 6 | 0% | 25.90 | 3.6011 | 0 | 28% | 49.90 | 0.2335 | 10 |
| `GB-LIVE32` | `/USB_P`, `/USB_N` | 0.32, 0.32 | 52% | 10.37 | 0.0736 | 0 | 0% | 10.30 | 0.0000 | 0 | 73% | 10.39 | 0.0000 | 0 |
| `nk3-nrf-mini` | `/D+`, `/D-` | 0.20, 0.25 | 0% | 14.81 | 0.0000 | 4 | 0% | 14.81 | 2.3291 | 4 | 1% | 16.50 | 8.5720 | 4 |

Lengths. *Outside* is how far the farthest net is outside its length, in millimetres.

| Board | Nets | Constraint, mm | guide: lengths | Outside | Skew | apart: lengths | Outside | Skew | original: lengths | Outside | Skew |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `isi-channel` | `/CH1_P`, `/CH1_N` | 54.85 to 55.35, skew 0.1 | 55.10, 55.10 | 0 | 0.0000 | 23.05, 51.32 | 31.80 | 28.268 | 55.10, 55.08 | 0 | 0.0125 |
| `isi-channel` | `/CH2_P`, `/CH2_N` | 149.60 to 150.10, skew 0.1 | 149.85, 149.85 | 0 | 0.0000 | 94.88, 94.75 | 54.85 | 0.1250 | 149.92, 149.80 | 0 | **0.1250** |
| `isi-channel` | `/CH3_P`, `/CH3_N` | 299.75 to 300.25, skew 0.1 | 300.00, 300.00 | 0 | 0.0000 | 94.75, 94.75 | 205.00 | 0.0000 | 300.00, 300.00 | 0 | 0.0000 |
| `udb-s` | `D+`, `D-` | skew 0.1 | 32.66, 32.66 | - | 0.0000 | 22.30, 25.90 | - | **3.6011** | 49.90, 49.67 | - | **0.2335** |
| `GB-LIVE32` | `/USB_P`, `/USB_N` | skew 0.1 | 10.37, 10.30 | - | 0.0736 | 10.30, 10.30 | - | 0.0000 | 10.39, 10.39 | - | 0.0000 |
| `nk3-nrf-mini` | `/D+`, `/D-` | skew 0.1 | 14.81, 14.81 | - | 0.0000 | 14.81, 12.48 | - | **2.3291** | 16.50, 7.93 | - | **8.5720** |

Skews and lengths in bold break their constraint. guide meets all nine, with 556 mm of
meanders on `isi-channel`, 1.4 mm on `udb-s` and 2.3 mm on `nk3-nrf-mini`; a skew of 0.0000
is under 50 nm. The hand routing breaks three, which its designers never asked of it: the skew
rules here are this test's. Completion, our DRC and KiCad's are as without pairs or lengths on
all four boards, and so are the times, to within the machine's noise: 0.21, 0.89, 33.4 and
20.7 s.

These tables are of the router before [zones](#pcb-zones-and-planes). With the boards' zones
kept and their nets routed through them, the default since, all nine constraints are still met,
with KiCad's DRC as clean: `udb-s`'s pair is coupled for 52% of its 28.08 mm, with 4 vias,
`GB-LIVE32`'s is unchanged, `nk3-nrf-mini`'s two nets come out at 16.56 mm with 5 vias and
0.6 mm of meander, and `isi-channel`'s channels are the same lengths, with no ground track
around them.

### What the results say

- **Coupling.** The two USB pairs that can get out side by side run so for about half their
  length: 46% on `udb-s`, against 28% by hand, and 52% on `GB-LIVE32`, against 73%. What isn't
  coupled is the fan-outs, near the pads, where each net goes its own way to the coupled run,
  and on `udb-s`, the stubs to the USB-C connector's second row of pins. Routed as two nets,
  neither pair has any length at its gap.
- **Pairs are routed last.** Each pair is routed side by side after everything else, in place of
  its two nets' routes, and kept only if it connects what they did, so no other net changes:
  every board's other nets are as before pairs. Routing pairs first did slightly better on
  `udb-s`, 60%, but lost `GB-LIVE32` a net ([the decision](architecture.md#decisions)).
- **What pairs cost.** `udb-s`'s pair takes 6 vias and 33 mm, where as two nets it took none and
  26 mm: the top layer is full by the time it is routed, and it goes down to an inner layer as
  two pairs of vias, which the hand routing does too, with 10. `GB-LIVE32`'s costs nothing.
- **Pairs not routed side by side.** `nk3-nrf-mini`'s USB pair runs from the edge connector into
  the 0.5 mm-pitch aQFN, where two tracks of 0.2 mm at a gap of 0.25 mm can't get out together,
  so it stays as two nets; a skew meander of 2.3 mm on the shorter matches them. `isi-channel`'s
  pairs are named as pairs but have their two nets on connectors 20 mm apart, beyond the 5 mm
  that two pads may be apart for a pair to start side by side, so they are routed as two nets,
  as their designer did.
- **Meanders.** `isi-channel`'s channels need up to 205 mm more than their shortest routes, and
  get it, each net to within 50 nm of its target, in meanders of up to 3 mm on their straight
  runs. Skews within pairs come out under 50 nm when they have to be met, and are left alone
  under the limit, as `GB-LIVE32`'s 0.074 mm: meanders are only added where a constraint asks.

### What this does not show

- Four boards, and few pairs: two coupled, all USB at 12 Mbit/s or less. No board has a bus of
  matched lengths with many nets, such as DDR, nor a pair through a BGA.
- The constraints are this test's, not the designers': none of these boards has custom rules.
- Coupling is measured, not impedance or crosstalk. Fan-outs, vias and meanders all change both,
  and nothing here models them.
- Meanders are trombones of square corners, on one side of a track at a time; there are no
  sawtooth or rounded meanders, and none on arcs.

## PCB zones and planes

Copper zones filled by `pcb::fill_zones` against KiCad's own fills, and the curated boards
routed with their zones' nets connected through them, against the same router with zones
ignored and against the hand routing, which used zones too. Measured on 2026-11-10 on the same
machine, one thread.

### How

```text
cargo test --release --features kicad curated_fills -- --nocapture             # our DRC, against saved fills
GUIDE_KICAD=1 cargo test --release --features kicad curated_fills -- --nocapture  # against KiCad 10's
GUIDE_ROUTE=1 GUIDE_KICAD=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_PLANES=0 cargo test --release --features kicad curated_boards_route -- --nocapture
```

[`zone_tests.rs`](../src/kicad/zone_tests.rs) keeps each board's tracks and vias, takes its
zones' fills out, fills them again, and checks the result with `drc::check`, which must find
nothing involving a fill. With `GUIDE_KICAD=1`, `kicad-cli pcb drc --refill-zones --save-board`
10.0.3 refills the same board, and its fills are compared with ours zone by zone and layer by
layer: the area of each, and the area that only one of the two covers. The board is also
written with our fills by `kicad::replace_routing_and_fills`, and checked by KiCad's DRC without
refilling. Teardrops, which KiCad keeps as zones, stay as they are.

Routing is [as above](#pcb-routing-of-the-curated-boards), on the boards stripped of tracks,
vias and fills, with the zones' outlines kept. The routed board is written with our fills, and
KiCad's DRC checks it twice: as written, and with `--refill-zones`, its own fills in place of
ours. Both must find no violation involving a routed item, and no missing connection on a
board with every net routed.

### Fills against KiCad's

| Board | Zones | KiCad's, mm² | Ours, mm² | Only one of the two, mm² | KiCad's DRC of our fills | Unconnected: KiCad's fills, ours |
| --- | --- | --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 1 | 948.1 | 948.4 | 0.26 (0.03%) | clean | 0, 0 |
| `TS4231-BOB` | 2 | 208.9 | 209.8 | 2.07 (0.99%) | clean | 0, 0 |
| `lisn` | 10 | 3756.2 | 3757.8 | 8.54 (0.23%) | clean | 0, 0 |
| `isi-channel` | 1 | 14750.4 | 14750.5 | 0.15 (0.00%) | clean | 0, 0 |
| `placebo` | 1 | 407.6 | 410.0 | 2.42 (0.59%) | clean | 0, 0 |
| `udb-s` | 1 | 734.8 | 736.7 | 1.94 (0.26%) | clean | 3, 3 |
| `GB-LIVE32` | 8 | 5186.9 | 5188.7 | 1.83 (0.04%) | clean | 0, 0 |
| `nk3-nrf-mini` | 4 | 218.9 | 218.7 | 0.67 (0.31%) | clean | 0, 0 |

`flea` has no zone. Our DRC finds nothing involving our fills on any board, and KiCad's no
error involving a zone. `udb-s`'s 3 unconnected items are the board's own, with KiCad's fills
too. What differs is mostly thermal spokes, where ours and KiCad's are laid out a little
differently on rotated and custom pads, and text, which we keep clear of as a box around it.
Getting there took what KiCad 10 does and doesn't:

- the board's edge is kept clear of by the edge clearance from its line, whatever the line's
  width, and holes by the hole clearance, not the zone's clearance, as the fills saved in the
  files, from KiCad 7, were: against those, ours differ by up to 9%;
- of two overlapping zones of one priority, the one with the greater UUID fills first;
- a spoke is kept only if its end is in the fill or in another spoke, and reaches past the
  pad's own size, not its box's, by the gap and half the minimum width.

### Routing through zones

| Board | Nets | Connections | DRC violations | Vias: routed → after cleanup (as tracks) (original) | Track, mm: routed → after cleanup (as tracks) (original) | Seconds (as tracks) | KiCad, our fills and refilled |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 6/6 | 8/8 | 0 | 0 → 0 (0) (0) | 93 → 93 (128) (115) | 0.06 (0.01) | 0 violations, 0 unconnected |
| `TS4231-BOB` | 8/8 | 13/13 | 0 | 1 → 0 (1) (10) | 29 → 31 (43) (39) | 0.11 (0.01) | 0 violations, 0 unconnected |
| `lisn` | 18/18 | 60/60 | 0 | 9 → 7 (10) (0) | 141 → 144 (329) (328) | 0.45 (0.08) | 0 violations, 0 unconnected |
| `isi-channel` | 7/7 | 29/29 | 0 | 2 → 0 (0) (0) | 461 → 453 (771) (1319) | 0.14 (0.21) | 0 violations, 0 unconnected |
| `placebo` | 23/23 | 50/50 | 0 | 21 → 9 (9) (10) | 250 → 292 (321) (286) | 0.39 (0.13) | 0 violations, 0 unconnected |
| `udb-s` | 8/8 | 33/33 | 0 | 15 → 11 (12) (24) | 143 → 149 (187) (172) | 1.35 (0.95) | 0 violations, 0 unconnected |
| `flea` | 24/24 | 79/79 | 0 | 56 → 52 (52) (31) | 1602 → 1607 (1607) (1704) | 4.92 (5.43) | 0 violations, 0 unconnected |
| `GB-LIVE32` | 79/79 | 197/197 | 0 | 255 → 165 (142) (140) | 2222 → 2283 (2705) (2315) | 24.4 (33.9) | 0 violations, 0 unconnected |
| `nk3-nrf-mini` | 29/30 | 82/83 | 0 | 63 → 55 (60) (60) | 231 → 249 (307) (349) | 19.4 (24.0) | 0 violations, 1 unconnected |
| `lpddr4-testbed` | 57/57 | 344/344 | 0 | 233 → 220 (-) (248) | 647 → 670 (-) (956) | 102 (-) | 0 violations, 0 unconnected |
| `ddr5-testbed` | 66/66 | 255/255 | 0 | 196 → 165 (-) (180) | 852 → 908 (-) (1119) | 114 (-) | 0 violations, 0 unconnected |

The two DRAM boards, added for M6, are [below](#pcb-routing-of-dram-boards), and weren't
routed as tracks. *As tracks* is the router with `Options::planes` off, which routes zones' nets as any others
and fills the zones after: its tracks and vias are exactly [the results
above](#pcb-routing-of-the-curated-boards), before zones, and its times are those recorded
there. Connections count what the zones join as made. The original's track is now comparable,
as it too leaves to zones what they join.

- **Completion** is as before: eight boards complete, and `nk3-nrf-mini` all but `/LED_R`. KiCad
  finds nothing wrong with any of them, with our fills or its own.
- **Track** is shorter on every board with zones, 3694 mm in all against 4791 as tracks and 4923
  by hand: ground and power are stubs to their planes. `isi-channel`'s ground goes to its plane
  on `In1.Cu`, and its track is now its three channels' 453 mm, before meanders.
- **Vias** are 23 more on `GB-LIVE32`, 165 against 142 as tracks and 140 by hand, as every
  ground and supply pad on the surface has a via of its own to its plane, which tracks between
  pads would share. On the other boards they are as many or fewer.
- **Time** is less on the two hard boards, where ground was the hardest net: 24 s against 34
  on `GB-LIVE32`, 19 against 24 on `nk3-nrf-mini`. Filling zones twice or more costs up to half
  a second on the small boards.

### What the cost of a plane does

The extra cost of another net's track inside a plane, as a fraction of its length,
`Options::plane_cost`:

| Plane cost | Boards complete | `GB-LIVE32` vias, mm, s | `nk3-nrf-mini` |
| --- | --- | --- | --- |
| 0 | 7 of 9 | 172, 2245, 8.5 | 78/83, `/3V3` in pieces |
| 1, the default | 8 of 9 | 165, 2283, 24.4 | 82/83 |
| 3 | 7 of 9 | 200, 2376, 43.3 | 80/83, `/SCL` unrouted |

With no cost, signals cut `nk3-nrf-mini`'s `/3V3` plane on `In2.Cu` in pieces that stubs can't
join again; with too much, they crowd the outer layers, and lose a signal there. The other
seven boards are the same at all three.

Placing the stubs first, as fixed copper before anything else is routed, rather than as a net's
routes to its plane that rip-up can move, completed `GB-LIVE32` in 14 s, but cost `nk3-nrf-mini`
three signals around its aQFN, 79 connections of 83.

### What this does not show

- Fills are compared on eight boards' zones, mostly simple: rectangles and a few polygons, no
  hatched fills, no zone with a fillet or chamfer of its corners, which KiCad applies to
  `TS4231-BOB`'s outlines and we don't.
- The planes are whole: no board here splits a layer between several nets' planes, beyond
  `GB-LIVE32`'s `In2.Cu`, so stubs rarely have to choose.
- Zones' fills are written for KiCad, which then checks them, but not for fabrication:
  nothing here is a Gerber.

## PCB routing of DRAM boards

M6's exit asks for "a real high-density board (DDR memory or equivalent)" routed "with its
constraints satisfied". These are the two DRAM boards of the [curated
set](../testdata/kicad/README.md), Antmicro's `lpddr4-testbed` and `ddr5-testbed`: each a DRAM
in a BGA, an LPDDR4 of 200 balls at 0.8 by 0.65 mm or an x8 DDR5 of 82 at 0.8 mm, broken out to
the 260 fingers of a SO-DIMM edge, on six layers, with 0.1 mm tracks and clearances, 0.4 and
0.45 mm vias, ground and power on planes, 57 and 66 nets to route, of up to 168 pads. Stripped
as [above](#pcb-routing-of-the-curated-boards), with their zones' outlines kept, and routed by
`pcb::route`. Measured on 2026-11-10 on the same machine, one thread.

### How

```text
GUIDE_ROUTE=1 GUIDE_KICAD=1 GUIDE_BOARDS=lpddr4-testbed,ddr5-testbed cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_KICAD=1 GUIDE_BOARDS=lpddr4-testbed,ddr5-testbed cargo test --release --features kicad curated_pairs -- --nocapture
```

The first routes them with the defaults and no constraints; the second with constraints, as
`.kicad_dru` custom rules that `kicad::length_rules` reads and `LengthRules::apply` gives the
design, and with [`Options::pairs_first`](../src/pcb/mod.rs). Neither board has rules of its
own, but their hand routing shows what their designers asked of it, and the rules ask the same:

- `lpddr4-testbed`: the data bus, its 16 `DQ`, 2 `DMI` and the two pairs of strobes, 22 nets,
  matched within 0.1 mm, and the command bus, 6 `CA`, `CKE`, `CS` and the clock pair, 10 nets,
  matched within 0.1 mm. By hand, they are 10.24 and 14.14 mm, within 0.01 and 0.04 mm. Its
  strobes and clock are named `_T`/`_C`, which KiCad doesn't take for pairs, so they aren't
  routed as pairs.
- `ddr5-testbed`: the data bus, 8 `DQ` and the pairs `DQS` and `TDQS`, 12 nets, and the
  command bus, 14 `CA`, `CS_N` and the pair `CLK`, 17 nets, each matched within 0.1 mm, and the
  three pairs at the width and gap of the hand routing, 0.1016 and 0.25 mm, by `track_width`
  and `diff_pair_gap` rules, where the board's only net class has 0.2 and 0.25. By hand, most
  of the data bus is at 20.40 mm and most of the command bus at 22.01, but `DQ2` is 0.56 mm
  longer and `CA12` and `CA13` 2.5 and 2.1 mm shorter.

Every result is checked by our DRC and connectivity, and KiCad 10.0.3's DRC, as above.

### Routing

| Board | Nets | Connections | DRC violations | Vias: routed → after cleanup (original) | Track, mm: routed → after cleanup (original) | Seconds | KiCad, our fills and refilled |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `lpddr4-testbed` | 57/57 | 344/344 | 0 | 233 → 220 (248) | 647 → 670 (956) | 102 | 0 violations, 0 unconnected |
| `ddr5-testbed` | 66/66 | 255/255 | 0 | 196 → 165 (180) | 852 → 908 (1119) | 114 | 0 violations, 0 unconnected |

Both are complete, with nothing wrong by KiCad's DRC, with our fills or KiCad's own, in under
two minutes, with fewer vias than by hand and less track, as the hand routing's buses are
meandered to their lengths and these have no lengths to meet.

### Constraints

With the rules above. *Coupled* is the share of a pair's length at its gap, as
[before](#pcb-differential-pairs-and-lengths); a skew in bold breaks its constraint.

| Board | Pair | guide: coupled | Length, mm | Skew | Vias | original: coupled | Length | Skew | Vias |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ddr5-testbed` | `DQS_P`, `DQS_N` | 73% | 22.96 | 0.0000 | 0 | 70% | 20.40 | 0.0000 | 4 |
| `ddr5-testbed` | `TDQS_P`, `TDQS_N` | 63% | 22.96 | 0.0000 | 2 | 15% | 20.40 | 0.0000 | 4 |
| `ddr5-testbed` | `CLK_P`, `CLK_N` | 64% | 21.54 | 0.0000 | 0 | 76% | 22.01 | 0.0000 | 4 |

| Board | Group | Nets | guide: lengths, mm | Skew | apart, no meanders: skew | original: lengths | Skew |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `lpddr4-testbed` | data | 22 | 11.10 | 0.000 | 4.563 | 10.23 to 10.24 | 0.010 |
| `lpddr4-testbed` | command | 10 | 15.23 | 0.000 | 4.425 | 14.13 to 14.17 | 0.044 |
| `ddr5-testbed` | data | 12 | 22.96 | 0.000 | 5.227 | 20.40, but `DQ2` 20.96 | **0.560** |
| `ddr5-testbed` | command | 17 | 21.54, but `CA3` unrouted | 0.000 of 16 | - | 22.01 to 22.26, but `CA12` 19.54, `CA13` 19.96 | **2.720** |

| Board | Nets | Connections | Pairs side by side | Constraints met | Meanders, mm | Seconds | KiCad |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `lpddr4-testbed` | 57/57 | 344/344 | - | 2 of 2 | 88.5 | 95 | 0 violations, 0 unconnected |
| `ddr5-testbed` | 63/66 | 252/255 | 3 of 3 | 2 of 2, of the nets routed | 154.0 | 150 | 0 violations, 3 unconnected |

**`lpddr4-testbed` meets its constraints; `ddr5-testbed` doesn't, and the exit is not met.**
`lpddr4-testbed` routes completely, both its buses matched to within 50 nm. On `ddr5-testbed`,
all three pairs run side by side, `TDQS` for 63% where the hand routing has 15%, and both buses
are matched to within 50 nm, but of the nets routed: with the pairs first and the nets of the
rules narrower, three connections are left, `RESET_N` and `CA3`, which the passes on the finer
lattice can't get in among the pairs and the rest, and a ground finger that no stub joins to its
plane. KiCad's DRC finds nothing wrong with any routed item. Before this round, at commit
`c87f814`, the same board left one net, `RESET_N`, had `TDQS` coupled for 4%, and its command
bus one net 3.5 mm short; `lpddr4-testbed`'s data bus one net 0.23 mm short. Coupling `TDQS`,
which crosses the board from its balls on top to its fingers below, takes room the others
needed: which nets are left changes with any change, as the end of this section
says, and every variation measured, below, left two to four.

### What it took

Before M6, both boards came out complete but for their ground, of 168 and 114 pads, which
failed whole, 9 and 7 connections short, in 100 and 120 s:

- **Budgets by connection.** A net's search budget, `Options::max_expansions`, is now for each
  64 connections, so that a net of 168 pads doesn't run out where one of 2 wouldn't. Nets of
  fewer, every net of the nine smaller boards, are as before.
- **Stubs one at a time.** A net of more than 64 terminals is searched a terminal at a time,
  the nearest its first terminal first, its plane if it has one, and each search with an
  eighth of the budget. A search for all the terminals at once computed a lower bound to each
  for every node it reached, which cost ground 15 to 50 s every time it was routed, and it was
  ripped up and routed again many times. `lpddr4-testbed` went from 225 to 100 s.
- **Partial routes.** A net with a plane, or of many pads, that its rip-up search can't route
  whole keeps what it reaches, searching from each terminal left to the tree on its own, rather
  than losing everything for one pad shut in, and its plane's pieces are joined after routing
  as before. A net of many pads that stays in pieces is unrouted, with its pieces kept.
- **A second fine pass.** Nets the pass on the finer lattice leaves get one more, every net free
  to be ripped up again: `ddr5-testbed`'s `CA12` came out so.
- **Planes joined after meanders.** Meanders may cut a plane on an outer layer, and they did on
  `ddr5-testbed`'s `VDD`, so its pieces are joined again after them as after routing.
- **Pairs first** ([`Options::pairs_first`](../src/pcb/mod.rs)), off by default: routed last, no
  pair of `ddr5-testbed` finds room through the buses around the BGA, even with the routes in
  its way ripped up and routed again, which was tried and dropped. Routed first, they do. And a
  pair whose end is in an array of pads that it can't pass between, as a BGA, fans out of the
  whole array apart before running side by side.
- **Meanders closer to a track's ends**, a track's width and clearance from them rather than
  twice that, and on the shortest nets of a group first: `lpddr4-testbed`'s data bus went from two nets short, by 0.6
  and 1.2 mm, to one, by 0.23 mm.
- **Room for meanders given back.** A net left short where its neighbours' meanders took the
  room has theirs taken out, is meandered first, and they after it; kept if fewer nets are
  short. That matched the last net of `lpddr4-testbed`'s data bus, 0.23 mm short, and of
  `ddr5-testbed`'s command bus, 3.5 mm short in the crowd on `F.Cu`, whose room the data bus's
  meanders had taken. Routing a short net again longer, through a waypoint where the shortest
  way through comes to the length it needs, was tried first and dropped: on both boards, of
  12 and 22 waypoints, no route through one came out, as its second half finds no way of its
  own beside the first in a bus.
- **Pairs across layers.** `TDQS`'s balls are on top and its fingers below, and its search for
  a centreline, whose vias cost 20 mm each, ran out of its 2 million nodes before it found
  one: its lower bound now counts the layer change it can't do without.
- **A way out for ground pins beside pairs.** With the pairs first, a ground finger beside
  `TDQS`'s was walled in by `TDQS`'s fan-out, and no stub could join it to its plane: a pad of a net with zones closer to a pair's pads than the pair's width and gap
  now keeps a place for a via free of the pairs, while they are routed.
- **Rules**: `kicad::length_rules` reads `track_width`, `diff_pair_gap` and
  `diff_pair_uncoupled` constraints too, and `LengthRules::apply` gives them to a design.

Neither board needed anything for blind, buried or micro vias, which neither allows or uses,
nor net topologies, as every signal of both is a point-to-point connection between the DRAM
and the edge; their BGAs needed no escape vias, as a 0.1 mm track passes between their balls.
Push-and-shove wasn't needed for completion without constraints: rip-up completes both.

What was tried for `ddr5-testbed`'s nets left with the pairs first, and dropped, each measured
on the board with its constraints:

| Variation | Nets left | Pairs side by side | Constraints met |
| --- | --- | --- | --- |
| As kept | `RESET_N`, `CA3`, a ground finger | 3 of 3 | 2 of 2, of the nets routed |
| No way out kept for ground pins | `RESET_N`, `CA7`, `TEN`, a ground finger | 3 of 3 | 2 of 2, of the nets routed |
| A way out kept for pins of nets with zones within 2 mm of a pair's | `RESET_N`, `TEN` | 2 of 3 | 2 of 2 |
| The same, and nets of more than 16 pads routed a terminal at a time from the start, planes' too | `RESET_N`, `CA9`, `TEN` | 2 of 3 | 2 of 2, of the nets routed |
| A second run with the nets left routed first of all, before the pairs | `CA11`, `CA5`, `DQS_P` | 2 of 3 | 2 of 2, of the nets routed |
| The same, with four or six passes on the finer lattice | `CA11`, `CA5` | 2 of 3 | 1 of 2 |
| Pairs that a net left finds in its way moved, and routed again after it | three or four | 3 of 3 | 2 of 2, of the nets routed |

Those that leave two nets lose a pair, and the second run doubles the time; moving pairs at
the end of routing failed every time, as by then no pair finds room again.

### What this does not show

- Two boards of one DRAM each, point to point: no fly-by bus of several chips, no DIMM, and no
  FPGA or SoC end with its own BGA.
- The constraints are this test's, from the hand routing, not the designers' rules, which the
  boards don't have; impedance, and lengths inside the packages, aren't modelled.
- Results are chaotic, as for the hard boards above: the same board with rules, whose pairs
  are narrower, loses nets that it routes without them, and every change above loses
  different ones.

## PCB routing against Freerouting

This is the comparison that M5's exit asks for, and M6's: `pcb::route` and Freerouting 2.5.0
on the eleven [curated boards](../testdata/kicad/README.md), each given the same stripped board,
both results measured by the same code and checked by KiCad 10.0.3's own DRC.

Measured on 2026-11-10 with rustc 1.97.1, on an AMD Ryzen Threadripper PRO 9995WX shared with
other work: Freerouting's runs on commit `8b0bf72`, and guide's again on commit `10a29e8`, after
nets of many pads came to be routed in part and completed. Freerouting is 2.5.0's jar on
Adoptium's JRE 25.0.5. It was first run on 2026-11-08, on commit `8871703`, before escape
routing, zones and pairs; `guide`'s numbers on eight of the nine boards it had then are the
same now, as the stripped boards here have no zones and no pair is routed side by side but
`udb-s`'s and `GB-LIVE32`'s, as before; `nk3-nrf-mini`'s are not, as below.

### How

```text
JAVA=jre/bin/java FREEROUTING_JAR=freerouting-2.5.0.jar scripts/freerouting/compare.sh OUT_DIR
```

[`scripts/freerouting/compare.sh`](../scripts/freerouting/compare.sh) does it all, and keeps
every file it makes in `OUT_DIR`:

1. **The same problem.** [`strip.py`](../scripts/freerouting/strip.py) loads each board in
   KiCad's `pcbnew`, takes out every track, arc and via and every copper zone, teardrops
   included, keeping rule areas, as the [routing test](#pcb-routing-of-the-curated-boards)
   does, and saves it. KiCad's own `ExportSpecctraDSN` then writes the DSN of that stripped
   board, as `testdata/kicad`'s DSN files were made. Copper zones go, not only their fills,
   because KiCad exports a zone as a plane, which Freerouting would take as poured copper to
   connect to, while neither router fills zones for the DRC.
2. **Freerouting** routes the DSN headless, with its settings and logs kept in `OUT_DIR` and
   analytics off. Its session is imported into the stripped board by KiCad's own
   `ImportSpecctraSES`, as "Import Specctra Session" does.
3. **guide** reads the same stripped `.kicad_pcb` with `kicad::read`, routes it with
   `pcb::route` and the default options, on one thread, and writes it back with
   `kicad::replace_routing`.
4. **The same measures.** `curated_boards_against_freerouting` in
   [`src/kicad/freerouting_tests.rs`](../src/kicad/freerouting_tests.rs) reads both routed
   boards with `kicad::read` and measures them alike: nets and connections completed, by
   islands of copper as in the routing test, vias, track length and our DRC. It runs
   `kicad-cli pcb drc` on both at error severity, and counts the violations that involve a
   track or a via, which on a stripped board are all the router's, and the missing
   connections. It runs only when the script sets `GUIDE_FREEROUTING`, so nothing in a normal
   test run needs Java or Freerouting.

Freerouting's runs:

- **default**: Freerouting's own settings. Its autorouter is single-threaded, and its optimizer
  here runs on 191 threads, one fewer than the machine has.
- **1-thread**: the optimizer on one thread too, to compare times with guide's one thread.
  `-mt 1` doesn't reach the optimizer in 2.5.0, so this sets `router.optimizer.max_threads=1`.
  Freerouting is deterministic: this run's sessions are the default's, byte for byte, on every
  board, and only the times differ.
- **long**: a longer run, on one thread. Freerouting's autorouter stops once its score hasn't
  improved for 10 passes, whatever `max_passes` says, so the run that can go on is the
  optimizer's: `router.optimizer.improvement_threshold=0` keeps it going while a pass improves
  at all, where the default stops below 2.5%.
- **tuned**: on one thread, with two things the DSN doesn't say given to Freerouting by its
  settings: the board's clearance between copper and the edge, and no automatic neckdown,
  which narrows tracks at pads below the board's minimum track width.

Times are of routing alone: for guide, `pcb::route`'s, and for Freerouting, the elapsed time of
its job, from its log, which leaves out starting Java, two to three seconds.

### What each is given

Both see the same pads, keepouts, edge and nets, but not the same rules. guide reads the
`.kicad_pcb` and its project; Freerouting reads what the DSN carries, as [the comparison of the
two readings](#pcb-dsn-designs-against-kicad-boards) lists. What matters here:

- **Not in the DSN**: the board's minimum track width and minimum hole, its edge clearance,
  pads' own clearances, solder mask openings, and differential pairs' gaps. Freerouting can't
  keep to rules it isn't given, and KiCad's DRC checks them all the same. Any KiCad user of
  Freerouting loses them, so they count here, but the `tuned` run gives Freerouting two of
  them, to see how much is that.
- **Approximated in the DSN**: custom pads are their convex hulls, rounded pads are polygons
  reaching a little past their arcs, and non-plated holes are keepouts grown by the hole
  clearance. These only make Freerouting's problem harder: `flea`'s DSN has 96 pairs of `U1`'s
  pads that overlap as hulls, and `nk3-nrf-mini`'s curved touch pad covers more than it does.
- **Only guide** keeps clear of text on copper, which the DSN doesn't have.

### Results

Freerouting's default run and its `tuned` run against guide. The violations are KiCad's, at
error severity, of tracks and vias: guide's results have none on any board, and our DRC finds
none in them either.

| Board | Router | Nets | Connections | KiCad violations | KiCad unconnected | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | guide | 6/6 | 8/8 | 0 | 0 | 0 | 128 | 0.01 |
| | Freerouting | 6/6 | 8/8 | 0 | 0 | 0 | 133 | 0.35 |
| | tuned | 6/6 | 8/8 | 0 | 0 | 0 | 140 | 0.28 |
| `TS4231-BOB` | guide | 8/8 | 13/13 | 0 | 0 | 1 | 43 | 0.01 |
| | Freerouting | 8/8 | 13/13 | 0 | 0 | 0 | 53 | 0.88 |
| | tuned | 8/8 | 13/13 | 0 | 0 | 0 | 53 | 1.21 |
| `lisn` | guide | 18/18 | 60/60 | 0 | 0 | 10 | 329 | 0.08 |
| | Freerouting | 18/18 | 60/60 | 0 | 0 | 6 | 408 | 8.60 |
| | tuned | 18/18 | 60/60 | 0 | 0 | 3 | 392 | 22.6 |
| `isi-channel` | guide | 7/7 | 29/29 | 0 | 0 | 0 | 771 | 0.22 |
| | Freerouting | 7/7 | 29/29 | 14 drill out of range | 0 | 14 | 805 | 7.16 |
| | tuned | 7/7 | 29/29 | 19: 17 drill out of range, 2 mask bridge | 0 | 17 | 834 | 6.71 |
| `placebo` | guide | 23/23 | 50/50 | 0 | 0 | 9 | 321 | 0.12 |
| | Freerouting | 23/23 | 50/50 | 1 edge clearance | 0 | 4 | 346 | 3.28 |
| | tuned | 23/23 | 50/50 | 0 | 0 | 11 | 349 | 13.3 |
| `udb-s` | guide | 8/8 | 33/33 | 0 | 0 | 12 | 187 | 0.98 |
| | Freerouting | 8/8 | 33/33 | 9 edge clearance | 0 | 14 | 189 | 2.16 |
| | tuned | 6/8 | 30/33 | 0 | 3 | 8 | 147 | 2.48 |
| `flea` | guide | 24/24 | 79/79 | 0 | 0 | 50 | 1588 | 4.37 |
| | Freerouting | 23/24 | 78/79 | 54: 50 track width, 4 mask bridge | 1 | 38 | 1684 | 16.8 |
| | tuned | 23/24 | 78/79 | 0 | 1 | 43 | 1667 | 19.8 |
| `GB-LIVE32` | guide | 79/79 | 197/197 | 0 | 0 | 142 | 2705 | 34.2 |
| | Freerouting | 79/79 | 197/197 | 4 track width | 0 | 168 | 2849 | 56.0; 485 on one thread |
| | tuned | 78/79 | 196/197 | 0 | 1 | 164 | 2859 | 32.2 |
| `nk3-nrf-mini` | guide | 25/30 | 72/83 | 0 | 11 | 54 | 297 | 123 |
| | Freerouting | 12/30 | 42/83 | 28 track width | 41 | 6 | 120 | 50.5 |
| | tuned | 11/30 | 36/83 | 3: 2 clearance, 1 mask bridge | 47 | 7 | 104 | 43.8 |
| `lpddr4-testbed` | guide | 57/57 | 344/344 | 0 | 0 | 130 | 1081 | 112 |
| | Freerouting | 50/57 | 337/344 | 75 track width | 7 | 120 | 1323 | 1020 |
| | tuned | 47/57 | 334/344 | 0 | 10 | 115 | 1235 | 550 |
| `ddr5-testbed` | guide | 65/66 | 254/255 | 0 | 1 | 142 | 1140 | 242 |
| | Freerouting | 51/66 | 240/255 | 37 track width | 15 | 121 | 1188 | 631 |
| | tuned | 58/66 | 245/255 | 0 | 10 | 139 | 1295 | 707 |

Freerouting's `1-thread` run gives the default's results on all but `lisn` and `isi-channel`,
which differ a little (12 and 17 vias), unlike in the first run, where all were the same byte
for byte; its times are the default's but on `GB-LIVE32`, 485 s. The `long` run changes
`isi-channel` (12 vias, 798 mm), `udb-s` (7 vias, 175 mm) and `lpddr4-testbed`, where it makes
fewer connections, 325, with 77 violations. KiCad also finds violations of no track or via on
some of the stripped boards themselves, such as pads too close on `flea`; they are the same
for both routers, and left out.

These stripped boards have no zones, as Freerouting would take a zone of the DSN as copper
poured, so ground and power are routed as tracks by both. The DRAM boards' designs put them
on planes, and routed so, with their zones, guide completes both, as
[above](#pcb-routing-of-dram-boards).

Before commit `10a29e8`, guide left the DRAM boards' supplies, `lpddr4-testbed`'s `VDDQ` of 43
pads and `ddr5-testbed`'s `VDD` of 22, unrouted, as they failed whole, and with them 42 and 21
connections: 302 and 233, against Freerouting's 337 and 240. A net of more than 16 pads that
rip-up can't route whole now keeps what it reaches, a terminal at a time, and the next pass on
the finer lattice completes it from there. `lpddr4-testbed` is now complete, and
`ddr5-testbed` has all but one ground connection. On the nine smaller boards, only
`nk3-nrf-mini` changes: 72 connections in place of 56, with 25 nets complete in place of 28, as
nets kept in part leave others less room, in 123 s in place of 43.

### What the results say

- **Completion.** guide completes nine of the eleven boards, Freerouting seven in its default
  run and five tuned: guide completes `flea` and `lpddr4-testbed` too, and `udb-s` and
  `GB-LIVE32` against the tuned run. On every board, guide makes as many connections as
  Freerouting's best run or more: on `nk3-nrf-mini` 72 against 42 and 36, on `lpddr4-testbed`
  all 344 against 337 and 334, and on `ddr5-testbed` 254 of 255 against 240 and 245, where it
  completes every net but ground, 65 of 66, against Freerouting's 51 and 58.
- **KiCad's DRC.** guide's results have no violation on any board. Freerouting's default run has
  some on eight of the eleven, 222 in all, nearly all of rules the DSN doesn't carry: tracks
  narrower than the board's minimum, from its neckdown at pads, edge clearances, mask openings,
  and `isi-channel`'s class vias, whose drill is under the board's minimum hole. Tuned, it has 22
  on two boards.
- **Track length.** guide's is shorter than Freerouting's, default and tuned, on every board
  but `nk3-nrf-mini`, where it makes many more connections, and `udb-s` against the tuned run,
  which leaves three out: on the seven boards both complete by default, 4484 mm against 4783,
  6%, and on the DRAM boards, with more connections, 18% and 4% shorter than the default run.
- **Vias.** guide has fewer than Freerouting's default on three boards, `isi-channel`, `udb-s`
  and `GB-LIVE32`, as many on one, and **more on seven**: `TS4231-BOB` by one, `lisn` 10 to 6,
  `placebo` 9 to 4, `flea` 50 to 38, `nk3-nrf-mini` with its many more connections, and the
  DRAM boards, 130 and 142 against 120 and 121, whose supplies, which Freerouting leaves in
  part, take a via at every pad.
- **Time, one thread each.** guide is faster on every board but `nk3-nrf-mini`: 2 to 100 times
  on the small ones, 1.7 times on `GB-LIVE32` against Freerouting on all threads and 15 times
  on one, and 9 and 2.6 times on the DRAM boards, where Freerouting takes 17 and 10 minutes.
  On `nk3-nrf-mini`, guide takes 123 s, against 50 for Freerouting, routing 30 more
  connections.

So on completion with no violation, guide ties Freerouting on the four boards that one of
Freerouting's runs completes with none, `GB-BRK-LINK-A`, `TS4231-BOB`, `lisn` and `placebo`,
and beats it on the other seven, where every run of Freerouting either makes fewer connections
or breaks rules, the DRAM boards included. It is shorter on all but `nk3-nrf-mini`, and faster
on all but that one, but has more vias on seven.

### The clearance violation only Freerouting finds

On `TS4231-BOB` as it comes, with its wiring, Freerouting reports one clearance violation that
neither KiCad nor our DRC finds: `FixedVia#106 [net GND] <-> R1.2`. The via is at (129.833069,
50.750919) mm, 0.8 mm across, and pad 2 of `R1` is a 0.8 × 0.95 mm rounded rectangle with
corners of 0.2 mm, at (130.625, 51.4). The via faces the pad's corner, and the gap between them
is 0.700228 − 0.2 − 0.4 = 0.100228 mm, 0.2 µm more than the clearance of 0.1 mm. KiCad, our DRC
and our DSN reading all pass it. The DSN's polygon for the pad reaches 0.761 µm past the corner's
arc at its vertices but falls short of it between them, and leaves a gap of about 103 µm there.

Freerouting checks clearances on shapes made of tiles, and takes a via as the octagon around its
circle. The via faces the pad 32° off the axis, where the octagon's diagonal side stands proud
of the circle, and that side comes within 82.4 µm of the pad polygon's nearest vertex, which
violates 100 µm. That predicts that the via must move 18 µm away from the pad, along the line
through the corner, for Freerouting to stop reporting it, where a circle would need nothing.
Moving it in the DSN, with its wires, step by step: Freerouting still reports it 17 µm away, and
no longer does 18 µm away. The violation is Freerouting's approximation, not the board's. It
errs safe, but it costs Freerouting room near every via, which is part of why it routes less of
`nk3-nrf-mini`.

### What this does not show

- Eleven boards, nine small and two dense, and one result of each router on each. Another
  board, or a changed option, can change who wins: routing the hard boards is chaotic.
- The boards are routed with no zones, for Freerouting's sake, which is not how the DRAM boards
  were designed: their ground goes to planes. Neither router is given their lengths or pairs
  here, which [below](#pcb-routing-of-dram-boards) has for guide alone.
- Freerouting is given a DSN, as KiCad users give it, and most of its violations are of rules
  the DSN doesn't carry. The `tuned` run gives it two of them, not all: it has no setting for
  pads' own clearances or masks.
- Freerouting's settings are its defaults and three variations. Others, such as its via costs
  or its `-us` and `-is` strategies, might do better. guide's are its defaults, which were
  tuned on these boards as [above](#what-changes-what), which favours guide.
- The machine was shared, and times move by 10% to 20% from run to run: guide's on `flea` was
  26 s in an earlier run.
- KiCad's DRC is the only judge of validity, and track length and vias the only measures of
  quality. Neither says how good a board is to make, or how its signals fare.

## Placement on synthetic problems

The roadmap's exit for generic placement (M7) asks for legal placements with wirelength close to
known reference solutions, deterministic like the router. Real placement benchmarks have no
known optimum, so these problems are made to have one, by `guide::place::synth`, and every
result is divided by it.

### How

Each generator lays out a reference placement first and then builds nets that are as short as
any net of their size can be: a net of `d` pins on distinct sites of a unit lattice spans at
least the least `w + h - 2` over boxes of `w` by `h` sites with `w * h >= d`, and every net of the
reference meets that bound. So the reference is optimal, and the optimum is a lower bound that
no legal placement can beat, which every run checks. Items are numbered in a shuffled order so
that nothing of the answer is left in the numbering.

| Family | Layout | Nets | Optimum |
| --- | --- | --- | --- |
| Grid | Full grid of sites | Each item to its right and upper neighbours | 1 per net |
| Grid, pads | The grid in a ring of fixed pads, on sites of another kind | The same, and each edge item to its pad | 1 per net |
| Grid, pads, macros of 4 | The same, with each column cut into rigid vertical macros of 4 | The same | 1 per net |
| Chain | 200 items on a 16 x 16 grid of sites, 56 left empty | Each item to the next | 199, a snake |
| Peko | Full grid of sites, optionally with pads, and with every eighth column for a second kind of item, like an FPGA's memory columns | One per item, as in Peko (Chang, Cong and Xie, 2003): 58% of 2 pins, 17% of 3, 8% of 4, 5% of 5, 8% of 6 to 10, 4% of 11 to 20, each in a box of least half-perimeter for its size | Sum of the bounds |
| Free space | 10 x 10 squares that can turn, pins at their centres, on a lattice of pitch 5, in an area that holds them exactly or with 20% to spare on each side | The grid's | 10 per net: squares that don't overlap have centres at least 10 apart |

The methods, all from `guide::place` with default options but for the seed:

- **Annealing**: `anneal`, from a random legal placement. VPR's schedule with `inner_num` 1, and
  30% of single items' moves sent to the median region of their nets.
- **Annealing, 10x moves**: the same with `inner_num` 10.
- **Analytic, legalized**: `place_analytic`'s global placement and legalization, before
  detailed placement (`AnalyticReport::legalized`).
- **Analytic, detailed**: its result, after detailed placement by annealing that starts cold,
  with a range limit of 8 steps and `inner_num` 3 (`AnnealOptions::detailed`).

Each family is run at seeds 1 to 3, one run at a time, on one thread of a shared 192-thread
machine. Ratios are the mean over the seeds, with the worst in brackets, and times are means in
seconds. `GUIDE_PLACE_SWEEP=quality cargo bench --bench place` prints the table.

### Results

| Problem | Items | Nets | Optimum | Annealing | Annealing, 10x moves | Analytic, legalized | Analytic, detailed | Annealing s | 10x s | Analytic s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Grid 16 x 16 | 256 | 480 | 480 | 1.798 (1.846) | 1.077 (1.198) | 1.000 (1.000) | 1.000 (1.000) | 0.032 | 0.29 | 0.023 |
| Grid 16 x 16, pads | 320 | 544 | 544 | 1.306 (1.415) | 1.000 (1.000) | 1.000 (1.000) | 1.000 (1.000) | 0.032 | 0.29 | 0.021 |
| Grid 32 x 32, pads | 1152 | 2112 | 2112 | 1.382 (1.487) | 1.115 (1.205) | 1.000 (1.000) | 1.000 (1.000) | 0.254 | 2.42 | 0.150 |
| Grid 16 x 16, pads, macros of 4 | 320 | 544 | 544 | 1.556 (1.886) | 1.000 (1.000) | 1.000 (1.000) | 1.000 (1.000) | 0.009 | 0.09 | 0.012 |
| Chain of 200 on 16 x 16 | 200 | 199 | 199 | 1.248 (1.296) | 1.049 (1.065) | 1.414 (1.432) | 1.040 (1.045) | 0.015 | 0.15 | 0.016 |
| Peko 16 x 16 | 256 | 272 | 471 | 1.485 (1.556) | 1.215 (1.240) | 1.504 (1.560) | 1.205 (1.231) | 0.066 | 0.66 | 0.065 |
| Peko 32 x 32, pads | 1152 | 1196 | 2020 | 1.646 (1.687) | 1.123 (1.132) | 1.462 (1.470) | 1.182 (1.186) | 0.518 | 5.15 | 0.453 |
| Peko 32 x 32, pads, typed columns | 1152 | 1196 | 2020 | 1.677 (1.745) | 1.218 (1.278) | 1.635 (1.688) | 1.222 (1.256) | 0.511 | 4.98 | 0.486 |
| Free space 8 x 8, full | 64 | 112 | 1120 | 1.438 (1.679) | 1.327 (1.402) | 1.143 (1.214) | 1.143 (1.214) | 0.015 | 0.17 | 0.023 |
| Free space 12 x 12, full | 144 | 264 | 2640 | 1.674 (1.936) | 1.280 (1.390) | 1.538 (1.538) | 1.189 (1.212) | 0.057 | 0.56 | 0.080 |
| Free space 12 x 12, 20% wider | 144 | 264 | 2640 | 1.665 (1.894) | 1.096 (1.265) | 1.564 (1.564) | 1.023 (1.068) | 0.049 | 0.46 | 0.073 |
| Geometric mean | | | | 1.525 | 1.132 | 1.271 | 1.088 | | | |

Every placement is legal by `place::check`, which shares no code with the placers.

Before the changes that closed M7, the same sweep gave this (commit `06dbc12`, same machine):

| Problem | Annealing | Annealing, 10x moves | Analytic, legalized | Analytic, detailed | Analytic s |
| --- | ---: | ---: | ---: | ---: | ---: |
| Grid 16 x 16 | 1.778 (1.904) | 1.499 (1.600) | 1.000 (1.000) | 1.000 (1.000) | 0.010 |
| Grid 16 x 16, pads | 1.496 (1.724) | 1.085 (1.158) | 1.000 (1.000) | 1.000 (1.000) | 0.008 |
| Grid 32 x 32, pads | 1.860 (2.014) | 1.188 (1.207) | 1.000 (1.000) | 1.000 (1.000) | 0.052 |
| Grid 16 x 16, pads, macros of 4 | 1.999 (2.239) | 1.420 (1.562) | 1.000 (1.000) | 1.000 (1.000) | 0.004 |
| Chain of 200 on 16 x 16 | 1.328 (1.387) | 1.057 (1.070) | 1.414 (1.432) | 1.060 (1.070) | 0.006 |
| Peko 16 x 16 | 1.444 (1.489) | 1.253 (1.291) | 1.504 (1.560) | 1.259 (1.283) | 0.017 |
| Peko 32 x 32, pads | 1.611 (1.683) | 1.186 (1.210) | 1.462 (1.470) | 1.267 (1.280) | 0.108 |
| Peko 32 x 32, pads, typed columns | 1.877 (1.936) | 1.298 (1.433) | 1.635 (1.688) | 1.325 (1.360) | 0.156 |
| Free space 8 x 8, full | 1.568 (1.625) | 1.259 (1.286) | 1.143 (1.214) | 1.143 (1.214) | 0.005 |
| Free space 12 x 12, full | 1.856 (2.030) | 1.451 (1.602) | 3.867 (3.867) | 1.702 (1.860) | 0.019 |
| Free space 12 x 12, 20% wider | 2.105 (2.227) | 1.268 (1.396) | 1.665 (1.665) | 1.188 (1.216) | 0.019 |
| Geometric mean | 1.704 | 1.262 | 1.390 | 1.161 | |

**Annealing against its moves.** `GUIDE_PLACE_SWEEP=effort` runs annealing alone at four values
of `inner_num`, without median moves and with 30% of them, and prints the mean ratio over the
three seeds:

| Problem | 0.3x | 1x | 3x | 10x | 0.3x, median | 1x, median | 3x, median | 10x, median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Grid 16 x 16 | 2.426 | 1.778 | 1.539 | 1.499 | 2.136 | 1.798 | 1.438 | 1.077 |
| Grid 16 x 16, pads | 2.086 | 1.496 | 1.221 | 1.085 | 1.464 | 1.306 | 1.185 | 1.000 |
| Grid 32 x 32, pads | 2.844 | 1.860 | 1.435 | 1.188 | 2.068 | 1.382 | 1.192 | 1.115 |
| Grid 16 x 16, pads, macros of 4 | 2.618 | 1.556 | 1.597 | 1.000 | 2.618 | 1.556 | 1.597 | 1.000 |
| Chain of 200 on 16 x 16 | 1.621 | 1.328 | 1.121 | 1.057 | 1.513 | 1.248 | 1.101 | 1.049 |
| Peko 16 x 16 | 1.680 | 1.444 | 1.313 | 1.253 | 1.767 | 1.485 | 1.350 | 1.215 |
| Peko 32 x 32, pads | 2.250 | 1.611 | 1.357 | 1.186 | 1.960 | 1.646 | 1.406 | 1.123 |
| Peko 32 x 32, pads, typed columns | 2.475 | 1.877 | 1.576 | 1.298 | 2.200 | 1.677 | 1.397 | 1.218 |
| Free space 8 x 8, full | 1.812 | 1.693 | 1.304 | 1.289 | 1.661 | 1.438 | 1.295 | 1.327 |
| Free space 12 x 12, full | 2.167 | 1.806 | 1.559 | 1.451 | 1.972 | 1.674 | 1.347 | 1.280 |
| Free space 12 x 12, 20% wider | 2.424 | 1.956 | 1.456 | 1.479 | 2.146 | 1.665 | 1.431 | 1.096 |
| Geometric mean | 2.186 | 1.662 | 1.399 | 1.242 | 1.928 | 1.525 | 1.333 | 1.132 |
| Moves per run, mean | 126982 | 417466 | 1241569 | 4122116 | 128082 | 416762 | 1246431 | 4157773 |
| Seconds, all 33 runs | 1.3 | 4.1 | 12.2 | 39.5 | 1.4 | 4.5 | 13.7 | 45.7 |

Median moves only move single items, so macros of 4, which hold every item of their grid, are
the same either way.

**Determinism.** The same seed gives the same placement: property tests check it on random
problems for every method, and a regression test pins the wirelengths of one problem. Nothing
runs in parallel inside a placer, so thread counts can't change anything; to check the claim
anyway, `GUIDE_PLACE_SWEEP=threads` runs all 33 instances of the table with every method on one
thread and then 16 at once, and gets the same wirelengths. Floating point is used only in a fixed
order, with `math::exp` for the acceptance test and Newton's method for a cube root; the row
legalizer is all integers.

### What changed, and what each change did

Measured with the quality sweep as each went in; the ratios are geometric means over the
families unless a family is named.

- **Rows after Abacus in free space.** Greedy legalization of 144 squares packed exactly full
  left gaps of half a square and fell back to packing that ignored the global placement, at 3.87
  times the optimum. Rows, along x and along y, with each unit in its nearest row or with rows
  filled in order, legalize the same upper bound (1.31 times the optimum) at 1.54, and detailed
  placement takes that to 1.22, from 1.70, or 1.19 with the stronger detailed placement below.
  With 20% to spare, rows win too: 1.56 legalized, from 1.67. On sites, legalization costs nothing on any of
  these problems, as spreading leaves every item on its own site already (the upper bound and the
  legalized wirelength are equal on every sites family), so a row legalizer there would gain
  nothing and was not built.
- **A unit in free space can always reach as far as it is wide.** Before, with the range limit
  at one pitch, half a square here, a square in a full area could only swap with a neighbour
  whose edge its moved centre happened to touch. On its own this changed little in these sweeps.
- **The acceptance rate counts only moves that were priced.** Moves abandoned as illegal, which
  macros and full free space make common, held the rate down and so shrank the range limit and
  cooled the schedule at the wrong pace. Counting only priced moves took annealing with 10 times
  the moves from 1.198 to 1.129, with median moves, and macros of 4 from 1.420 to 1.000.
- **Median moves**, 30% of single items' moves: 1.662 to 1.525 at the default effort and 1.242 to
  1.132 at ten times it. 50% did about as well as 30%, and 10% less well.
- **Macros push the macros they land on** back the other way, and single items go to the nearest
  sites the shifts leave free. On the grid of macros this finds the same trades as before, as
  only macros of one shape fit each other's room, so the gain there is the rate's.
- **Stronger detailed placement.** Starting from the legalized placement, annealing at a
  twentieth of the moves' standard deviation with a range limit of 8 steps and `inner_num` 3,
  instead of 3 steps and 1, takes Peko from 1.26-1.33 to 1.18-1.22, for four times the time
  (0.45 s against 0.12 s on 1152 items, the time of annealing alone at its default effort).
  Starting hotter does worse: at the moves' full standard deviation the annealer undoes the
  global placement and, at this effort, can't rebuild it (1.18 to 1.25 geometric mean). With
  `inner_num` 10 it reaches 1.13-1.14 on the large Peko problems, in 1.4 to 1.5 s.
- **Global swap**, after FastPlace-DP (each item tries every place in the median region of its
  nets, and takes the best swap), was tried before and after detailed annealing, with windows of
  3 to 9 steps, and changed the geometric mean by at most 0.005, mostly for the worse, so it was
  left out: detailed annealing already finds what it finds.
- **VPR's schedule**, checked against Betz and Rose (1997): the temperature factors (0.5, 0.9,
  0.95, 0.8 by acceptance rate), the range limit's update towards 44% acceptance, the first
  temperature at 20 standard deviations and the exit at 0.005 of the cost per net are VPR's.
  Lower first temperatures, of 1 and 3 standard deviations, did no better at the default effort
  (1.544 against 1.525).

### What the results say

- **M7's exit is met.** The threshold taken is that the analytic flow, the default, lands
  within 25% of the optimum on average on every family and within 10% over all of them. It is at
  most 22% above on average (Peko with typed columns; 26% at worst over the seeds), 0% on every
  grid, with or without pads and macros, 4% on the chain, 2% to 19% in free space, and 8.8% as a
  geometric mean.
- **Analytic placement is the better default.** It is as good as annealing with ten times its
  moves on Peko problems (18% to 22% against 12% to 22% above the optimum), better everywhere
  else, and takes a seventh to a sixteenth of the time, about what annealing takes at its
  default effort.
- **Annealing alone needs effort.** At VPR's `inner_num` of 1 it is 25% to 80% above the
  optimum, and at 10 it is 0% to 33% above. Grids are hard for it, as for any annealer: regions
  settle into the grid's pattern at different orientations, and undoing the walls between them
  takes moves that only lose at first.
- **For context**, Peko's authors found the placers of 2003 far from the optimum on their
  problems, which were much larger, of tens of thousands of cells and more. Small problems are
  easier, so that is no comparison.
- **What is left in free space** is the grid without pads packed exactly full, 8 x 8, where two
  seeds of three end 21% above the optimum. Its quadratic placement has nothing to hold it, collapses to
  a point, and the first spreading decides its shape, which here comes out sheared, each row a
  step to the side of the one below. That is legal and locally optimal: undoing it takes moving
  whole diagonals, which neither swaps nor detailed annealing do.

### What this does not show

- These are synthetic problems, in which every net is as short as its size allows. Real
  netlists have nets that can't all be short at once, and the ratio to a known optimum says
  nothing about how a placement routes. The roadmap's measure for FPGA placement is wirelength
  and delay after routing, against VPR, which is M8's: see
  [FPGA placement against VPR](#fpga-placement-against-vpr).
- They are small: at most 1152 items. Neither placer has been tuned or profiled at the sizes of
  real FPGAs, and annealing, at `units^(4/3)` moves per temperature, grows quickly.
- Congestion feedback is only tested for doing what it should on a tiny case; nothing here
  measures what it does to routability.
- Free-space problems are squares of one size. Mixed sizes, which PCBs have, are only tested for
  legality, and rows as tall as the tallest unit waste room when sizes differ much.
