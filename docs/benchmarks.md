# Benchmarks

This is the start of the benchmark report that the [roadmap](../ROADMAP.md) asks for. Most of
it covers the engine, on synthetic instances that this repository generates. Those are
useful for tuning, for catching regressions and for checking what the docs say. They are **not
evidence about real designs**, and the [comparisons that would be](#fpga-and-pcb-comparisons)
have not been done. The one comparison on real designs so far is of the
[clearance check against KiCad's](#pcb-clearance-checks-against-kicad), on real boards.

## Running

```text
cargo bench --bench engine                             # criterion timings, about a minute
GUIDE_ENGINE_SWEEP=options cargo bench --bench engine  # option tables, about a minute
GUIDE_ENGINE_SWEEP=refine cargo bench --bench engine   # refinement after negotiation, seconds
GUIDE_ENGINE_SWEEP=expansion cargo bench --bench engine  # time per expansion, about 4 minutes
GUIDE_ENGINE_SWEEP=timing cargo bench --bench engine   # timing-driven routing, about a minute
GUIDE_ENGINE_SWEEP=all cargo bench --bench engine      # all four sweeps
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

**These are synthetic, and not a comparison with VPR.** No real VTR design can be routed yet:
the graph of a real VTR architecture needs VPR to generate it, and VPR was not available. So
the fabric, its delays and its netlist are all made up here, as `benches/engine.rs` describes.

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

**Not yet done**, but for the first row. None of the comparisons of results in the
[roadmap](../ROADMAP.md#measuring-best) has been run, so there are no results for the others.

| Domain | Compared against | Benchmarks | Status |
| --- | --- | --- | --- |
| FPGA routing | VPR, nextpnr | VTR benchmarks, Titan23 | Not yet done |
| FPGA placement | VPR, nextpnr | VTR benchmarks, ISPD 2016/2017 contest sets | Not yet done |
| PCB clearance checks | KiCad 10.0.3 | [Curated open-source KiCad boards](../testdata/kicad/README.md) | [Done](#pcb-clearance-checks-against-kicad) |
| PCB routing | Freerouting | The same boards | Not yet done |
| PCB placement | Hand-placed originals | The same boards | Not yet done |
| ASIC global routing | Published contest results | ISPD 2007/2008 | Not yet done |

## PCB clearance checks against KiCad

This is the comparison that M4's exit criterion asks for: `geom::drc::check`, on the
[curated boards](../testdata/kicad/README.md) as `kicad::read` reads them, against
`kicad-cli pcb drc` from KiCad 10.0.3, finding by finding.

### How

Nine real boards, of two to four layers, from 38 items to over 1200. Each is checked
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

In all, 26 checks: KiCad made 3450 findings and we made 3469, of which 3448 are the same. Of
the 23 differences, 2 are KiCad's only and 21 ours only, and each is explained below, from
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

**Arc** (3, on `placebo`: ours only). KiCad's `SHAPE_ARC::NearestPoints`, in `shape_arc.cpp`,
gets two cases wrong. Against a circle, such as a via or a round pad, it considers each end of
the arc only if `sliceContainsPoint` accepts it, which tests the end's angle in floating point
and can refuse the arc's own end. Against another arc, the loops that measure each arc's ends
against the other arc assign the running minimum without comparing with it, so a later, farther
candidate replaces a nearer one. The three are an arc and a round pad 0.2753 mm apart, under
two sets of rules, and two arcs 0.3856 mm apart, under a clearance of 0.4 mm. For the first,
KiCad measures 0.4723 mm, which is the distance from the arc's start, where its end is nearer;
with the same arc written the other way round, KiCad measures 0.2753 mm. Each of the three is
checked again by KiCad itself, with those arcs split into segments within 5 nm of them, and
KiCad then reports it.

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
