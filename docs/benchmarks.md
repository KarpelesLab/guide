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
checked by KiCad, and [their zones' fills](#pcb-zones-and-planes) against KiCad's, and
[the same boards placed again](#pcb-placement-of-the-curated-boards) from nothing, and
[engineering changes](#engineering-changes), routed and placed again incrementally, on generated
problems and VTR's circuits, and [placement and routing in a
loop](#pcb-placement-and-routing-in-a-loop) on the curated boards, with engineering changes and
pin swaps there, and [ASIC global routing](#asic-global-routing-against-the-ispd-contests) on the
ISPD 2007 and 2008 contests' designs, against their published results.
[Placement](#placement-on-synthetic-problems) is also measured on synthetic problems whose
optimum is known. Outside electronics, [multi-agent path
finding](#multi-agent-path-finding-on-the-movingai-benchmarks) is measured on the MovingAI
benchmarks against CBSH2-RTC, EECBS and LaCAM3.

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
| FPGA placement and routing in a loop, LUT rotation | The one-pass flow, checked by VPR 9.0.0 | VTR's MCNC circuits and Verilog suite | [Done](#fpga-placement-and-routing-in-a-loop) |
| FPGA placement | nextpnr | VTR benchmarks, ISPD 2016/2017 contest sets | Not yet done |
| FPGA routing | reticle's own router, both checked by reticle down to the decoded bitstream | reticle's designs on the XC7A35T, LFE5U-12F and GW2A-18 | [Done](#routing-reticles-designs) |
| FPGA placement | reticle's own placer, both routed and checked by reticle down to the decoded bitstream | reticle's designs on the LFE5U-12F, XC7A35T and its synthetic iCE40 | [Done, but for the 7 series' carry chains](#placing-reticles-designs) |
| PCB clearance checks | KiCad 10.0.3 | [Curated open-source KiCad boards](../testdata/kicad/README.md) | [Done](#pcb-clearance-checks-against-kicad) |
| PCB DSN import, SES export | KiCad 10.0.3's reading of the same boards, and its session import | The same boards | [Done](#pcb-dsn-designs-against-kicad-boards) |
| PCB routing | Freerouting 2.5.0, both checked by KiCad 10.0.3 | The same boards | [First results](#pcb-routing-against-freerouting) |
| PCB placement | Hand-placed originals, both routed by `guide` and checked by KiCad 10.0.3 | The same boards | [Done](#pcb-placement-of-the-curated-boards) |
| ASIC global routing | Published contest results, NTHU-Route 2.0, NCTU-GR 2.0, FastRoute 4.1 | ISPD 2007/2008 | [Done](#asic-global-routing-against-the-ispd-contests) |

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

Measured on 2026-11-10 with rustc 1.97.1, on the same shared machine, at `3a9562d`, which
prices congestion in placement by default, with the router as `1ba0591` tuned it. The placer
came with `24d4e8b`; [congestion](#congestion) below compares the two, with and without
congestion, on the same router.

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
  each temperature and delays from `fpga::PlaceDelayModel`, and a congestion term, the
  expected use of each channel above 0.6 of its tracks, weighed as the bounding box is.
  Constant and clock nets are left out, as VPR leaves them out.
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
| alu4 | 106 | 13 | 5320 | 5174 | 0.973 | 4.98964 | 5.28341 | 1.059 | 0.080 | 0.139 | 1.738 | 0 | yes |
| apex2 | 126 | 14 | 8242 | 7946 | 0.964 | 5.81286 | 5.9124 | 1.017 | 0.110 | 0.246 | 2.236 | 0 | yes |
| apex4 | 105 | 13 | 6734 | 6769 | 1.005 | 5.30224 | 5.46923 | 1.031 | 0.100 | 0.130 | 1.300 | 0 | yes |
| bigkey | 92 | 16 | 4499 | 3259 | 0.724 | 3.19105 | 3.44405 | 1.079 | 0.270 | 0.276 | 1.022 | 0 | yes |
| clma | 435 | 23 | 32117 | 31976 | 0.996 | 8.66055 | 8.99639 | 1.039 | 1.140 | 1.703 | 1.494 | 0 | yes |
| des | 102 | 18 | 5227 | 4797 | 0.918 | 4.37046 | 4.75988 | 1.089 | 0.260 | 0.126 | 0.485 | 0 | yes |
| diffeq | 102 | 13 | 3921 | 3785 | 0.965 | 6.43054 | 6.44803 | 1.003 | 0.120 | 0.143 | 1.192 | 0 | yes |
| dsip | 93 | 16 | 4931 | 4065 | 0.824 | 3.18883 | 3.44405 | 1.080 | 0.310 | 0.286 | 0.923 | 0 | yes |
| elliptic | 242 | 18 | 12132 | 11833 | 0.975 | 7.4606 | 8.03756 | 1.077 | 0.540 | 0.835 | 1.546 | 0 | yes |
| ex1010 | 322 | 20 | 27552 | 27245 | 0.989 | 7.05556 | 6.97623 | 0.989 | 0.520 | 0.893 | 1.717 | 0 | yes |
| ex5p | 98 | 12 | 4942 | 4811 | 0.973 | 4.47718 | 4.58361 | 1.024 | 0.080 | 0.173 | 2.162 | 0 | yes |
| frisc | 251 | 18 | 15146 | 14847 | 0.980 | 10.0229 | 10.3463 | 1.032 | 0.470 | 0.902 | 1.919 | 0 | yes |
| misex3 | 100 | 12 | 5175 | 5082 | 0.982 | 4.84801 | 5.16713 | 1.066 | 0.070 | 0.155 | 2.214 | 0 | yes |
| pdc | 332 | 21 | 25399 | 25710 | 1.012 | 6.92036 | 7.46194 | 1.078 | 0.720 | 1.292 | 1.794 | 0 | yes |
| s298 | 84 | 12 | 4089 | 4009 | 0.980 | 7.44269 | 7.18847 | 0.966 | 0.050 | 0.074 | 1.480 | 0 | yes |
| s38417 | 397 | 22 | 14984 | 13645 | 0.911 | 5.80932 | 5.78534 | 0.996 | 0.710 | 1.204 | 1.696 | 0 | yes |
| s38584.1 | 360 | 21 | 14456 | 14506 | 1.003 | 5.23191 | 5.61382 | 1.073 | 0.960 | 1.433 | 1.493 | 0 | yes |
| seq | 112 | 13 | 7217 | 7181 | 0.995 | 4.98507 | 5.146 | 1.032 | 0.120 | 0.270 | 2.250 | 0 | yes |
| spla | 265 | 19 | 17307 | 16924 | 0.978 | 6.63208 | 6.57809 | 0.992 | 0.430 | 0.878 | 2.042 | 0 | yes |
| tseng | 112 | 13 | 3158 | 3021 | 0.957 | 6.15771 | 6.22049 | 1.010 | 0.130 | 0.188 | 1.446 | 0 | yes |
| Geomean |  |  |  |  | 0.953 |  |  | 1.036 |  |  | 1.523 |  |  |

Routed by VPR's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 40 | 38 | 0.950 | 52 | 5.31783 | 4.99275 | 0.939 | 7291 | 6702 | 0.919 | yes |
| apex2 | 54 | 54 | 1.000 | 70 | 5.80687 | 5.76134 | 0.992 | 12151 | 11661 | 0.960 | yes |
| apex4 | 56 | 54 | 0.964 | 72 | 5.38635 | 5.34477 | 0.992 | 10212 | 9492 | 0.929 | yes |
| bigkey | 34 | 34 | 1.000 | 44 | 3.30692 | 3.2681 | 0.988 | 6415 | 4968 | 0.774 | yes |
| clma | 72 | 72 | 1.000 | 94 | 8.69463 | 8.47252 | 0.974 | 44365 | 43891 | 0.989 | yes |
| des | 34 | 34 | 1.000 | 44 | 4.46945 | 4.41387 | 0.988 | 6744 | 5989 | 0.888 | yes |
| diffeq | 30 | 30 | 1.000 | 40 | 6.09481 | 6.12015 | 1.004 | 5218 | 4893 | 0.938 | yes |
| dsip | 34 | 34 | 1.000 | 44 | 3.17889 | 3.43943 | 1.082 | 7198 | 6132 | 0.852 | yes |
| elliptic | 52 | 48 | 0.923 | 68 | 7.58148 | 7.51865 | 0.992 | 16865 | 16178 | 0.959 | yes |
| ex1010 | 86 | 88 | 1.023 | 112 | 6.89706 | 6.65474 | 0.965 | 40817 | 40064 | 0.982 | yes |
| ex5p | 44 | 44 | 1.000 | 58 | 4.62135 | 4.56112 | 0.987 | 6964 | 6930 | 0.995 | yes |
| frisc | 58 | 56 | 0.966 | 76 | 10.0072 | 9.74947 | 0.974 | 21617 | 20613 | 0.954 | yes |
| misex3 | 46 | 44 | 0.957 | 60 | 4.82071 | 5.06325 | 1.050 | 6920 | 6821 | 0.986 | yes |
| pdc | 72 | 72 | 1.000 | 94 | 7.06044 | 7.52518 | 1.066 | 36688 | 37224 | 1.015 | yes |
| s298 | 40 | 40 | 1.000 | 52 | 7.26292 | 6.64124 | 0.914 | 5886 | 5695 | 0.968 | yes |
| s38417 | 34 | 30 | 0.882 | 44 | 5.80047 | 5.49631 | 0.948 | 18574 | 16694 | 0.899 | yes |
| s38584.1 | 40 | 38 | 0.950 | 52 | 5.05609 | 5.2779 | 1.044 | 19108 | 19186 | 1.004 | yes |
| seq | 54 | 58 | 1.074 | 70 | 4.87201 | 5.00909 | 1.028 | 10428 | 10166 | 0.975 | yes |
| spla | 60 | 62 | 1.033 | 78 | 6.52939 | 6.29578 | 0.964 | 25425 | 24433 | 0.961 | yes |
| tseng | 26 | 24 | 0.923 | 34 | 5.71256 | 5.64603 | 0.988 | 4210 | 4101 | 0.974 | yes |
| Geomean |  |  | 0.981 |  |  |  | 0.993 |  |  | 0.944 |  |

Routed by guide's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 36 | 36 | 1.000 | 52 | 4.8465 | 4.99054 | 1.030 | 6951 | 6473 | 0.931 | yes |
| apex2 | 52 | 50 | 0.962 | 70 | 5.77742 | 5.59581 | 0.969 | 11739 | 11518 | 0.981 | yes |
| apex4 | 52 | 50 | 0.962 | 72 | 5.27845 | 5.27103 | 0.999 | 9352 | 9312 | 0.996 | yes |
| bigkey | 34 | 34 | 1.000 | 44 | 3.2653 | 3.25733 | 0.998 | 6263 | 4800 | 0.766 | yes |
| clma | 68 | 64 | 0.941 | 94 | 8.66512 | 8.4878 | 0.980 | 42652 | 42064 | 0.986 | yes |
| des | 34 | 32 | 0.941 | 44 | 4.46377 | 4.43359 | 0.993 | 6619 | 5934 | 0.897 | yes |
| diffeq | 28 | 28 | 1.000 | 40 | 6.10312 | 5.79765 | 0.950 | 4809 | 4594 | 0.955 | yes |
| dsip | 36 | 34 | 0.944 | 44 | 3.17582 | 3.4161 | 1.076 | 6917 | 5948 | 0.860 | yes |
| elliptic | 46 | 46 | 1.000 | 68 | 7.54636 | 7.51422 | 0.996 | 15326 | 14769 | 0.964 | yes |
| ex1010 | 76 | 80 | 1.053 | 112 | 6.82146 | 6.63487 | 0.973 | 38845 | 39077 | 1.006 | yes |
| ex5p | 44 | 42 | 0.955 | 58 | 4.54943 | 4.34903 | 0.956 | 6671 | 6585 | 0.987 | yes |
| frisc | 54 | 52 | 0.963 | 76 | 9.59249 | 9.66988 | 1.008 | 19669 | 19202 | 0.976 | yes |
| misex3 | 42 | 40 | 0.952 | 60 | 4.78367 | 4.89592 | 1.023 | 6861 | 6590 | 0.961 | yes |
| pdc | 64 | 66 | 1.031 | 94 | 7.05184 | 7.02664 | 0.996 | 34864 | 35500 | 1.018 | yes |
| s298 | 34 | 36 | 1.059 | 52 | 7.35173 | 6.64345 | 0.904 | 5232 | 5190 | 0.992 | yes |
| s38417 | 32 | 28 | 0.875 | 44 | 5.51245 | 5.30108 | 0.962 | 17667 | 16098 | 0.911 | yes |
| s38584.1 | 36 | 36 | 1.000 | 52 | 4.87983 | 5.21921 | 1.070 | 18154 | 18504 | 1.019 | yes |
| seq | 54 | 52 | 0.963 | 70 | 4.794 | 4.88102 | 1.018 | 9903 | 9956 | 1.005 | yes |
| spla | 54 | 54 | 1.000 | 78 | 6.51736 | 6.21489 | 0.954 | 24047 | 23121 | 0.961 | yes |
| tseng | 26 | 26 | 1.000 | 34 | 5.68422 | 5.61643 | 0.988 | 4050 | 3909 | 0.965 | yes |
| Geomean |  |  | 0.979 |  |  |  | 0.991 |  |  | 0.955 |  |

#### VTR's Verilog benchmarks

Blocks are CLBs, then multipliers and memories, and chains are the carry chains placed as
macros.

Placement, as VPR measures both:

| Circuit | Blocks | Grid | VPR BB | guide BB | / VPR | VPR est. CPD, ns | guide est. CPD, ns | / VPR | VPR, s | guide, s | / VPR | Chains | VPR accepts guide's |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 7600 / 32 / 167 | 103 | 1043923 | 1049071 | 1.005 | 76.166 | 75.898 | 0.996 | 341.290 | 222.173 | 0.651 | 166 | yes |
| LU8PEEng | 2232 / 8 / 44 | 57 | 238874 | 236192 | 0.989 | 75.9908 | 77.3803 | 1.018 | 33.930 | 16.875 | 0.497 | 70 | yes |
| arm_core | 1023 / 0 / 24 | 39 | 141706 | 142991 | 1.009 | 18.1898 | 18.2025 | 1.001 | 8.490 | 9.989 | 1.177 | 10 | yes |
| bgm | 2786 / 11 / 0 | 63 | 250115 | 233104 | 0.932 | 20.6448 | 19.2035 | 0.930 | 30.110 | 21.894 | 0.727 | 38 | yes |
| blob_merge | 619 / 0 / 0 | 31 | 45944 | 46982 | 1.023 | 15.2449 | 15.6769 | 1.028 | 2.520 | 2.937 | 1.165 | 0 | yes |
| boundtop | 97 / 0 / 0 | 14 | 1935 | 1787 | 0.924 | 3.36481 | 3.48744 | 1.036 | 0.190 | 0.099 | 0.521 | 0 | yes |
| ch_intrinsics | 68 / 0 / 1 | 12 | 825 | 795 | 0.964 | 2.18081 | 2.38504 | 1.094 | 0.060 | 0.029 | 0.483 | 0 | yes |
| diffeq1 | 32 / 5 / 0 | 16 | 5160 | 4990 | 0.967 | 15.9904 | 16.2014 | 1.013 | 0.240 | 0.098 | 0.408 | 4 | yes |
| diffeq2 | 21 / 7 / 0 | 18 | 5097 | 4861 | 0.954 | 12.3169 | 12.2575 | 0.995 | 0.160 | 0.077 | 0.481 | 4 | yes |
| mcml | 7302 / 27 / 159 | 101 | 718639 | 688920 | 0.959 | 47.0709 | 46.6894 | 0.992 | 327.680 | 212.368 | 0.648 | 471 | yes |
| mkDelayWorker32B | 473 / 0 / 45 | 50 | 15924 | 13431 | 0.843 | 8.181 | 6.68033 | 0.817 | 1.860 | 0.591 | 0.318 | 2 | yes |
| mkPktMerge | 31 / 0 / 15 | 28 | 9125 | 8058 | 0.883 | 4.09928 | 4.24256 | 1.035 | 0.390 | 0.145 | 0.372 | 0 | yes |
| mkSMAdapter4B | 189 / 0 / 5 | 20 | 11771 | 11836 | 1.006 | 4.67723 | 4.72287 | 1.010 | 0.790 | 0.537 | 0.680 | 7 | yes |
| or1200 | 258 / 1 / 2 | 27 | 31226 | 31835 | 1.020 | 8.49305 | 8.78375 | 1.034 | 2.590 | 1.433 | 0.553 | 13 | yes |
| raygentop | 123 / 8 / 0 | 19 | 11237 | 10840 | 0.965 | 4.53 | 4.62372 | 1.021 | 0.570 | 0.424 | 0.744 | 1 | yes |
| sha | 155 / 0 / 0 | 17 | 11670 | 11226 | 0.962 | 9.56668 | 9.73811 | 1.018 | 0.400 | 0.591 | 1.477 | 9 | yes |
| spree | 64 / 1 / 3 | 14 | 7318 | 7295 | 0.997 | 10.858 | 11.1119 | 1.023 | 0.270 | 0.174 | 0.644 | 2 | yes |
| stereovision0 | 725 / 0 / 0 | 34 | 40349 | 37371 | 0.926 | 3.44133 | 3.59736 | 1.045 | 3.630 | 2.381 | 0.656 | 0 | yes |
| stereovision1 | 770 / 40 / 0 | 40 | 79581 | 77292 | 0.971 | 5.13017 | 5.48943 | 1.070 | 4.160 | 4.355 | 1.047 | 0 | yes |
| stereovision2 | 1977 / 324 / 0 | 104 | 311596 | 324314 | 1.041 | 14.6044 | 13.5574 | 0.928 | 30.010 | 8.085 | 0.269 | 312 | yes |
| stereovision3 | 14 / 0 / 0 | 7 | 456 | 446 | 0.978 | 2.65898 | 2.71335 | 1.020 | 0.010 | 0.012 | 1.200 | 0 | yes |
| Geomean |  |  |  |  | 0.966 |  |  | 1.004 |  |  | 0.635 |  |  |

Routed by VPR's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 124 | 124 | 1.000 | 162 | 76.2861 | 75.8454 | 0.994 | 1310007 | 1312732 | 1.002 | yes |
| LU8PEEng | 92 | 92 | 1.000 | 120 | 76.1097 | 75.8584 | 0.997 | 325298 | 321825 | 0.989 | yes |
| arm_core | 104 | 114 | 1.096 | 136 | 18.6216 | 18.7268 | 1.006 | 184148 | 185223 | 1.006 | yes |
| bgm | 70 | 68 | 0.971 | 92 | 20.4561 | 19.7437 | 0.965 | 366218 | 348820 | 0.952 | yes |
| blob_merge | 58 | 62 | 1.069 | 76 | 15.2323 | 15.5491 | 1.021 | 66256 | 68345 | 1.032 | yes |
| boundtop | 42 | 40 | 0.952 | 54 | 3.55486 | 3.47891 | 0.979 | 3124 | 2942 | 0.942 | yes |
| ch_intrinsics | 42 | 38 | 0.905 | 54 | 2.49575 | 2.49995 | 1.002 | 1416 | 1310 | 0.925 | yes |
| diffeq1 | 60 | 50 | 0.833 | 78 | 17.0822 | 17.2194 | 1.008 | 8635 | 8565 | 0.992 | yes |
| diffeq2 | 46 | 40 | 0.870 | 60 | 12.9774 | 13.0262 | 1.004 | 9780 | 9579 | 0.979 | yes |
| mcml | 124 | 136 | 1.097 | 162 | 46.3566 | 44.2597 | 0.955 | 921573 | 893405 | 0.969 | yes |
| mkDelayWorker32B | 38 | 36 | 0.947 | 50 | 8.43017 | 6.84149 | 0.812 | 22576 | 19729 | 0.874 | yes |
| mkPktMerge | 46 | 34 | 0.739 | 60 | 4.26319 | 4.12434 | 0.967 | 13566 | 12671 | 0.934 | yes |
| mkSMAdapter4B | 50 | 58 | 1.160 | 64 | 4.82225 | 4.83551 | 1.003 | 17892 | 18578 | 1.038 | yes |
| or1200 | 92 | 88 | 0.957 | 120 | 8.64458 | 8.76104 | 1.013 | 43336 | 44224 | 1.020 | yes |
| raygentop | 58 | 62 | 1.069 | 76 | 4.74311 | 4.75893 | 1.003 | 18983 | 18438 | 0.971 | yes |
| sha | 68 | 70 | 1.029 | 88 | 10.0346 | 9.86047 | 0.983 | 15449 | 15141 | 0.980 | yes |
| spree | 62 | 64 | 1.032 | 80 | 11.2813 | 11.301 | 1.002 | 12039 | 11859 | 0.985 | yes |
| stereovision0 | 48 | 48 | 1.000 | 62 | 3.60577 | 3.60688 | 1.000 | 55531 | 51878 | 0.934 | yes |
| stereovision1 | 80 | 74 | 0.925 | 104 | 5.57953 | 5.56013 | 0.997 | 114217 | 111555 | 0.977 | yes |
| stereovision2 | 68 | 66 | 0.971 | 88 | 15.8253 | 13.8848 | 0.877 | 422434 | 433455 | 1.026 | yes |
| stereovision3 | 30 | 30 | 1.000 | 40 | 2.65817 | 2.70126 | 1.016 | 783 | 717 | 0.916 | yes |
| Geomean |  |  | 0.977 |  |  |  | 0.980 |  |  | 0.973 |  |

Routed by guide's router:

| Circuit | Min W, VPR's placement | Min W, guide's | / VPR | W | CPD, VPR's placement, ns | CPD, guide's, ns | / VPR | Wire, VPR's placement | Wire, guide's | / VPR | Routes legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 116 | 120 | 1.034 | 162 | 76.099 | 75.6934 | 0.995 | 1304833 | 1311170 | 1.005 | yes |
| LU8PEEng | 84 | 86 | 1.024 | 120 | 75.675 | 75.6256 | 0.999 | 324858 | 322272 | 0.992 | yes |
| arm_core | 100 | 104 | 1.040 | 136 | 18.5984 | 18.7146 | 1.006 | 184526 | 184714 | 1.001 | yes |
| bgm | 70 | 68 | 0.971 | 92 | 20.4639 | 19.6743 | 0.961 | 370151 | 352617 | 0.953 | yes |
| blob_merge | 60 | 62 | 1.033 | 76 | 15.1614 | 15.5387 | 1.025 | 67081 | 69587 | 1.037 | yes |
| boundtop | 42 | 40 | 0.952 | 54 | 3.55264 | 3.47781 | 0.979 | 3154 | 2929 | 0.929 | yes |
| ch_intrinsics | 42 | 38 | 0.905 | 54 | 2.45235 | 2.50282 | 1.021 | 1449 | 1329 | 0.917 | yes |
| diffeq1 | 54 | 46 | 0.852 | 78 | 17.0899 | 17.2624 | 1.010 | 8644 | 8607 | 0.996 | yes |
| diffeq2 | 42 | 38 | 0.905 | 60 | 13.0064 | 13.0119 | 1.000 | 9882 | 9459 | 0.957 | yes |
| mcml | 112 | 114 | 1.018 | 162 | 46.3666 | 44.2199 | 0.954 | 919519 | 889016 | 0.967 | yes |
| mkDelayWorker32B | 40 | 36 | 0.900 | 50 | 8.45975 | 6.83042 | 0.807 | 22082 | 19355 | 0.877 | yes |
| mkPktMerge | 46 | 38 | 0.826 | 60 | 4.29197 | 4.12545 | 0.961 | 13358 | 12523 | 0.937 | yes |
| mkSMAdapter4B | 50 | 54 | 1.080 | 64 | 4.80594 | 4.7995 | 0.999 | 18076 | 18417 | 1.019 | yes |
| or1200 | 76 | 76 | 1.000 | 120 | 8.63019 | 8.84103 | 1.024 | 43888 | 44607 | 1.016 | yes |
| raygentop | 54 | 56 | 1.037 | 76 | 4.73979 | 4.90751 | 1.035 | 18799 | 18383 | 0.978 | yes |
| sha | 56 | 58 | 1.036 | 88 | 10.0324 | 9.85715 | 0.983 | 15476 | 15134 | 0.978 | yes |
| spree | 62 | 62 | 1.000 | 80 | 11.2802 | 11.3206 | 1.004 | 12076 | 11910 | 0.986 | yes |
| stereovision0 | 48 | 46 | 0.958 | 62 | 3.60134 | 3.60356 | 1.001 | 55464 | 52052 | 0.938 | yes |
| stereovision1 | 78 | 72 | 0.923 | 104 | 5.45065 | 5.54574 | 1.017 | 114442 | 111247 | 0.972 | yes |
| stereovision2 | 60 | 62 | 1.033 | 88 | 15.7777 | 13.8782 | 0.880 | 419730 | 430911 | 1.027 | yes |
| stereovision3 | 30 | 30 | 1.000 | 40 | 2.66237 | 2.70126 | 1.015 | 756 | 702 | 0.929 | yes |
| Geomean |  |  | 0.975 |  |  |  | 0.983 |  |  | 0.971 |  |

### What the results say

- **After routing, `guide`'s placements are better than VPR's, which is M8's exit for FPGAs.**
  With VPR's router, channels are 1.9% narrower on MCNC and 2.3% narrower on the Verilog suite,
  critical paths 0.7% and 2.0% shorter, with 5.6% and 2.7% less wire. With `guide`'s router,
  channels 2.1% and 2.5% narrower, critical paths 0.9% and 1.7% shorter, with 4.5% and 2.9%
  less wire.
- **Before routing**, by VPR's own measures, `guide`'s placements need 4.7% less wire on MCNC
  and 3.4% less on the Verilog suite, and VPR's delay model puts their critical paths 3.6%
  longer on MCNC and 0.4% longer on the Verilog suite. Routing undoes that MCNC difference: see
  [the delay model](#what-it-took-1) below.
- **Channel width is still where single circuits do worst**, against VPR's placement with seed
  1: mkSMAdapter4B needs 16% wider channels (58 tracks against 50 with VPR's router), mcml 10%,
  arm_core 10%, blob_merge 7% and LU32PEEng the same; with `guide`'s router, 8%, 2%, 4%, 3% and
  3%. Some of that is VPR's luck: with seeds 2 and 3, VPR's own placements of mcml need 134
  and 138 tracks, and of arm_core 112 and 110, against 124 and 104 with seed 1. See
  [congestion](#congestion).
- **The largest gains are on timing**: stereovision2's critical path is 12% shorter (13.88 ns
  against 15.83 ns) and mkDelayWorker32B's 19% shorter, with VPR's router and less wire on
  mkDelayWorker32B. The worst are dsip, 8% longer, and seq, 3% longer, both with less wire.
- **Time**: 0.64 times VPR's on the Verilog suite, 222 s against 341 s on LU32PEEng, and 1.52
  times on MCNC, where both take fractions of a second and the global placement's fixed cost
  counts.

### Congestion

The placer as `24d4e8b` added it had no congestion term, and needed channels as wide as VPR's
placements on average, but up to 16% wider on single circuits. VPR 9.0.0's placer has none
either; VPR's newer congestion-aware placement adds one of this kind. `guide` now estimates,
each temperature, how much of each channel the placement will use, by RUDY with VPR's crossing
counts against the tracks the graph has ([how](algorithms.md#fpga-placement)), and charges each
net for the channels of its box that are above 0.6 of their tracks.

**Before and after**, at `3a9562d` with `--congestion-factor 0` and with the default, on the
same packing, router and harness as above, as geometric means of the ratios to VPR's
placement:

| Suite | Measure | Before | After |
| --- | --- | --- | --- |
| MCNC | BB / VPR, before routing | 0.955 | 0.953 |
| MCNC | Est. CPD / VPR, before routing | 1.041 | 1.036 |
| MCNC | Time / VPR | 1.58 | 1.52 |
| MCNC | Min W / VPR, VPR's router | 0.987 | **0.981** |
| MCNC | CPD / VPR, VPR's router | 0.998 | 0.993 |
| MCNC | Wire / VPR, VPR's router | 0.955 | 0.944 |
| MCNC | Min W / VPR, `guide`'s router | 0.981 | **0.979** |
| MCNC | CPD / VPR, `guide`'s router | 0.998 | 0.991 |
| MCNC | Wire / VPR, `guide`'s router | 0.957 | 0.955 |
| Verilog | BB / VPR, before routing | 0.967 | 0.966 |
| Verilog | Est. CPD / VPR, before routing | 1.000 | 1.004 |
| Verilog | Time / VPR | 0.59 | 0.64 |
| Verilog | Min W / VPR, VPR's router | 1.001 | **0.977** |
| Verilog | CPD / VPR, VPR's router | 0.980 | 0.980 |
| Verilog | Wire / VPR, VPR's router | 0.973 | 0.973 |
| Verilog | Min W / VPR, `guide`'s router | 0.986 | **0.975** |
| Verilog | CPD / VPR, `guide`'s router | 0.980 | 0.983 |
| Verilog | Wire / VPR, `guide`'s router | 0.971 | 0.971 |

The circuits that needed the widest channels, by minimum width with VPR's router and then with
`guide`'s:

| Circuit | VPR's placement | `guide`'s, before | After | VPR's placement, `guide`'s router | `guide`'s, before | After |
| --- | --- | --- | --- | --- | --- | --- |
| LU32PEEng | 124 | 126 | 124 | 116 | 122 | 120 |
| LU8PEEng | 92 | 96 | 92 | 84 | 88 | 86 |
| mcml | 124 | 138 | 136 | 112 | 112 | 114 |
| arm_core | 104 | 110 | 114 | 100 | 104 | 104 |
| blob_merge | 58 | 66 | 62 | 60 | 66 | 62 |
| mkSMAdapter4B | 50 | 58 | 58 | 50 | 56 | 54 |
| or1200 | 92 | 96 | 88 | 76 | 80 | 76 |
| stereovision3 | 30 | 36 | 30 | 30 | 36 | 30 |

**Seeds.** A single circuit's minimum width moves by up to 20% between seeds, of either placer,
so single runs say little about it. On the 39 circuits but LU32PEEng and mcml, `guide` placed
with five seeds, with and without congestion, and VPR with three, each placement routed by
VPR's router to its minimum width. Against VPR's placement with seed 1, as the tables above
have it, and against the geometric mean of VPR's three seeds for each circuit:

| Placer | Seeds | Min W / VPR seed 1, each seed | Mean | Min W / mean of VPR's seeds | Mean |
| --- | --- | --- | --- | --- | --- |
| VPR | 1, 2, 3 | 1, 0.988, 0.991 | 0.993 | 1.007, 0.995, 0.998 | 1.000 |
| `guide`, without congestion | 1 to 5 | 0.990, 0.995, 0.991, 1.011, 0.993 | 0.996 | 0.997, 1.002, 0.998, 1.018, 0.999 | 1.003 |
| `guide`, with congestion | 1 to 5 | 0.976, 0.973, 0.977, 0.991, 0.986 | **0.981** | 0.983, 0.980, 0.983, 0.997, 0.993 | **0.987** |

So congestion takes 1.5% off the channel width, on every seed, more than the means move between
seeds, about 0.5%; and VPR's seed 1 happened to be its worst of three, by 0.7%. By circuit, with
seeds 1 to 3 of VPR's and 1 to 5 of `guide`'s (1 and 2 on LU32PEEng, and 1 to 3 with
congestion on mcml):

| Circuit | VPR | `guide`, without | `guide`, with |
| --- | --- | --- | --- |
| LU32PEEng | 124, 124, 124 | 126, 124 | 124, 120 |
| mcml | 124, 134, 138 | 138, 138 | 136, 136, 142 |
| LU8PEEng | 92, 94, 92 | 96, 90, 88, 94, 92 | 92, 92, 94, 90, 88 |
| arm_core | 104, 112, 110 | 110, 110, 110, 108, 114 | 114, 108, 110, 108, 112 |
| blob_merge | 58, 60, 60 | 66, 64, 64, 64, 64 | 62, 62, 62, 62, 62 |
| mkSMAdapter4B | 50, 50, 48 | 58, 56, 54, 56, 54 | 58, 56, 56, 56, 52 |
| or1200 | 92, 98, 82 | 96, 114, 98, 106, 94 | 88, 90, 90, 108, 104 |

LU32PEEng, mcml, LU8PEEng and arm_core are now within the spread of VPR's own seeds. blob_merge
and mkSMAdapter4B are not: they need 2 to 10 tracks more than VPR's placements on every seed.
Both have more wire before routing than VPR's, 2.3% and 0.6% more, and placed for wirelength
alone, without timing, they needed 60 and 56 tracks, so timing costs them wire, more than
`guide`'s estimate misses congestion: on blob_merge a factor of 3 takes the expected peak use
from 0.90 to 0.79 of the tracks, below VPR's placement's 0.82, and the width stays at 64.

**Weights**, on the 39 circuits, one seed but where said, as geometric means of the ratio to
VPR's placement with seed 1, with VPR's router, and with BB and `guide`'s own estimate of the
critical path before routing:

| Setting | Min W / VPR | BB / VPR | Est. CPD / VPR, `guide`'s model |
| --- | --- | --- | --- |
| No congestion, 5 seeds | 0.996 | 0.957 | 0.978 |
| **Factor 1, threshold 0.6** (the default), 5 seeds | **0.981** | 0.955 | 0.982 |
| Factor 0.5, threshold 0.6 | 0.983 | 0.959 | 0.982 |
| Factor 2, threshold 0.6 | 0.975 | 0.958 | 0.982 |
| Factor 3, threshold 0.6 | 0.983 | 0.957 | 0.980 |
| Factor 1, threshold 0.5 | 0.980 | 0.956 | 0.977 |
| Factor 1, threshold 0.7 | 0.983 | 0.958 | 0.979 |
| Inflation to 2, no factor, 3 seeds | 0.978 | 0.955 | 0.982 |
| Factor 1 and inflation to 1.5 | 0.984 | 0.956 | 0.980 |
| Factor 1 and inflation to 2, 3 seeds | 0.981 | 0.956 | 0.982 |
| Factor 1 and inflation to 3 | 0.978 | 0.957 | 0.978 |
| Factor 2 and inflation to 2 | 0.971 | 0.960 | 0.983 |
| Factor 1, `timing_tradeoff` 0.4 | 0.983 | 0.943 | 0.989 |
| Factor 1, first range limit 16 | 0.977 | 0.958 | 0.978 |
| Factor 1, first temperature 0.1 | 0.984 | 0.957 | 0.979 |
| Factor 1, `inner_num` 1.5 | 0.981 | 0.955 | 0.979 |

- **The factor and the threshold** are flat from 0.5 to 2 and from 0.5 to 0.7: single seeds of
  them differ by less than seeds of one setting do, as factor 1 and inflation to 2 shows, at
  0.971, 0.992 and 0.980 on three seeds. A threshold of 0.6 of a device 1.3 times as wide as
  VPR needs is 0.78 of the tracks at VPR's minimum.
- **Inflating blocks in global placement** where channels are congested, `max_inflation`, does
  as much alone as the annealing term does, and nothing on top of it, so it stays off.
- **The term does more the further annealing can move blocks.** On mcml, annealing hot from a
  random placement goes from 154 tracks to 126 with the term, and the default flow with a first
  range limit of 32 and a temperature of 0.2 to 130, against 136; but both have 6% to 13% more
  wire and 8% longer critical paths before routing, and on the 39 a range of 16 did no better
  than 8.
- **Time**: the estimate is made again each temperature, in time linear in nets and tiles, and a
  net's term is four lookups in a summed-area table. Side by side, three times each, placement
  took 0% to 15% longer on LU8PEEng, bgm, stereovision2 and clma, and no longer on mcml, within
  the noise of a shared machine.

### How the default came about

Before congestion was priced, and with the router before `1ba0591` tuned it, on the 39
circuits but LU32PEEng and mcml, which take hours of routing, two other settings went through
the same comparison. Each row has the geometric means of the ratios to VPR's placement,
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

- One packing. The tables have one seed per circuit for each placer; [congestion](#congestion)
  has five of `guide`'s and three of VPR's on 39 circuits, with VPR's router only.
- Placement times are of runs a few days apart on a shared machine, VPR's from the routing
  comparisons; on MCNC they are fractions of a second, where fixed costs count.
- Clock regions: VTR's architectures have none, so nothing here needs them. Titan23 and the
  ISPD contest sets were not run, and neither was nextpnr.
- `guide` reads the device from VPR's graph rather than from the architecture's layout, and
  follows only blocks whose type has the name of a tile type, which is how VTR's architectures
  have it.

## FPGA placement and routing in a loop

This is M9 on FPGAs, "measurable completion or quality gains over the one-pass flow on the
hardest benchmarks": placement and routing as one loop, where what the router finds congested
goes back into the placer, and pin permutation, where the netlist permits it. Both are measured
against the one-pass flow of [FPGA placement against VPR](#fpga-placement-against-vpr), which
places once and routes once, on the same circuits, packings, architectures and harness, with
VPR 9.0.0 checking every placement and every route.

Measured on 2026-11-11 with rustc 1.97.1, on the same shared machine, with the loop as `e167386`
added it; `ca9e117` added an option that is off by default. The one-pass placements with seed 1
are those of the tables above.

### How

- **The loop** is `fpga::place_route_loop` at one channel width
  ([how](algorithms.md#placement-and-routing-in-a-loop)): `guide`'s router routes the
  placement, timing-driven, with at most 50 iterations, as the search for the narrowest channel
  does; if that is not legal, the router's history of overuse, added up over every iteration and
  every round, and how much more of each channel it used than RUDY expected, go into the
  placer's estimate of congestion; the blocks within 6 tiles of what the best iteration overused
  are placed again by `fpga::refine_placement`, annealing from where they are at a twentieth of
  the moves' standard deviation over 4 steps, timing-driven, the rest fixed; and the router routes
  again, up to 3 times.
- **Narrowing** is `scripts/vtr/loop.sh`: from the one-pass placement and its narrowest channel
  with `guide`'s router, W, the loop runs at W - 2; if it ends with a legal routing, it runs again
  at W - 4 from the placement that routed, and so on, until it fails. The placement it ends with
  is the last that routed, or the one-pass placement if none did.
- **Measures** are those of the one-pass flow, by `place-circuit.sh` on the loop's placement:
  VPR loads it with every block fixed, both routers find their narrowest channel on it, by VPR's
  binary search and by `guide`'s from VPR's width, VPR checks `guide`'s routes at that width, and
  both route it at the one-pass flow's relaxed width, 1.3 times VPR's narrowest on its own
  placement, for the critical path, by VPR's analysis, and the wire. Ratios are of the loop over
  the one-pass flow on the same seed, so below 1 is better for the loop.
- **Seeds**: the one-pass placement with seeds 1, 2 and 3, each followed by the loop, on every
  circuit of both suites but LU32PEEng and mcml, whose searches for the narrowest channel take
  hours, and which are below the tables.
- **Time** is the loop's own, routing and placing again in `vtr_loop`, against the one-pass
  placement's; it leaves out reading files and VPR writing graphs of each width.

```text
scripts/vtr/place.sh DIR BASE                         # the one-pass flow, as above
scripts/vtr/loop.sh DIR BASE LOOP                     # the loop, from it
SUITE=verilog BLIF=BLIF scripts/vtr/loop.sh DIR BASE2 LOOP2
python3 scripts/vtr/loop_tables.py BASE LOOP [BASE LOOP...]
scripts/vtr/rotate.sh DIR BASE                        # LUT rotation, below
python3 scripts/vtr/rotate_tables.py BASE
```

### Results

**Every placement is legal and every route checks**: VPR loaded every placement the loop ended
with, every block fixed, and VPR passed every route of `guide`'s of it, at the narrowest channel
and at the relaxed width. Below, "guide" and "VPR" say whose router found the narrowest channel,
on the placement of the one-pass flow with seed 1 and on the loop's from it; the first column is
VPR's router on VPR's own placement, for reference, and critical path and wire are VPR's router's
at the relaxed width.

#### MCNC

| Circuit | VPR's placement, VPR | One pass, guide | Loop, guide | / one pass | One pass, VPR | Loop, VPR | / one pass | CPD / one pass, VPR | Wire / one pass, VPR | Place, s | Loop, s | Legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | 40 | 36 | 34 | 0.944 | 38 | 40 | 1.053 | 1.005 | 1.007 | 0.1 | 2.1 | yes |
| apex2 | 54 | 50 | 50 | 1.000 | 54 | 54 | 1.000 | 1.000 | 1.000 | 0.2 | 1.8 | yes |
| apex4 | 56 | 50 | 48 | 0.960 | 54 | 54 | 1.000 | 1.008 | 0.994 | 0.1 | 3.1 | yes |
| bigkey | 34 | 34 | 34 | 1.000 | 34 | 34 | 1.000 | 1.000 | 1.000 | 0.3 | 2.0 | yes |
| clma | 72 | 64 | 62 | 0.969 | 72 | 70 | 0.972 | 0.994 | 0.989 | 1.7 | 22.9 | yes |
| des | 34 | 32 | 32 | 1.000 | 34 | 34 | 1.000 | 1.000 | 1.000 | 0.1 | 6.4 | yes |
| diffeq | 30 | 28 | 26 | 0.929 | 30 | 28 | 0.933 | 1.012 | 0.987 | 0.1 | 1.5 | yes |
| dsip | 34 | 34 | 34 | 1.000 | 34 | 34 | 1.000 | 1.000 | 1.000 | 0.3 | 2.2 | yes |
| elliptic | 52 | 46 | 42 | 0.913 | 48 | 46 | 0.958 | 0.989 | 0.972 | 0.8 | 13.6 | yes |
| ex1010 | 86 | 80 | 72 | 0.900 | 88 | 84 | 0.955 | 1.045 | 0.992 | 0.9 | 28.0 | yes |
| ex5p | 44 | 42 | 40 | 0.952 | 44 | 44 | 1.000 | 0.967 | 0.966 | 0.2 | 2.0 | yes |
| frisc | 58 | 52 | 48 | 0.923 | 56 | 54 | 0.964 | 1.020 | 0.988 | 0.9 | 9.9 | yes |
| misex3 | 46 | 40 | 38 | 0.950 | 44 | 44 | 1.000 | 0.949 | 0.975 | 0.2 | 2.3 | yes |
| pdc | 72 | 66 | 62 | 0.939 | 72 | 72 | 1.000 | 0.953 | 0.982 | 1.3 | 26.2 | yes |
| s298 | 40 | 36 | 32 | 0.889 | 40 | 36 | 0.900 | 1.093 | 0.971 | 0.1 | 2.6 | yes |
| s38417 | 34 | 28 | 26 | 0.929 | 30 | 30 | 1.000 | 0.996 | 0.990 | 1.2 | 6.0 | yes |
| s38584.1 | 40 | 36 | 34 | 0.944 | 38 | 38 | 1.000 | 1.008 | 0.978 | 1.4 | 8.5 | yes |
| seq | 54 | 52 | 48 | 0.923 | 58 | 54 | 0.931 | 1.000 | 0.972 | 0.3 | 3.5 | yes |
| spla | 60 | 54 | 50 | 0.926 | 62 | 58 | 0.935 | 1.044 | 0.987 | 0.9 | 20.6 | yes |
| tseng | 26 | 26 | 24 | 0.923 | 24 | 24 | 1.000 | 0.994 | 1.027 | 0.2 | 1.4 | yes |
| Geomean |  |  |  | 0.945 |  |  | 0.979 | 1.004 | 0.989 |  |  |  |

#### VTR's Verilog benchmarks

| Circuit | VPR's placement, VPR | One pass, guide | Loop, guide | / one pass | One pass, VPR | Loop, VPR | / one pass | CPD / one pass, VPR | Wire / one pass, VPR | Place, s | Loop, s | Legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| LU8PEEng | 92 | 86 | 84 | 0.977 | 92 | 88 | 0.957 | 0.997 | 0.998 | 16.9 | 829.2 | yes |
| arm_core | 104 | 104 | 102 | 0.981 | 114 | 104 | 0.912 | 0.999 | 0.991 | 10.0 | 266.5 | yes |
| bgm | 70 | 68 | 66 | 0.971 | 68 | 66 | 0.971 | 1.000 | 1.002 | 21.9 | 123.5 | yes |
| blob_merge | 58 | 62 | 60 | 0.968 | 62 | 62 | 1.000 | 0.987 | 0.978 | 2.9 | 38.7 | yes |
| boundtop | 42 | 40 | 36 | 0.900 | 40 | 36 | 0.900 | 0.980 | 1.043 | 0.1 | 0.7 | yes |
| ch_intrinsics | 42 | 38 | 36 | 0.947 | 38 | 36 | 0.947 | 1.001 | 1.086 | 0.0 | 0.2 | yes |
| diffeq1 | 60 | 46 | 44 | 0.957 | 50 | 50 | 1.000 | 1.000 | 0.970 | 0.1 | 3.5 | yes |
| diffeq2 | 46 | 38 | 38 | 1.000 | 40 | 40 | 1.000 | 1.000 | 1.000 | 0.1 | 4.1 | yes |
| mkDelayWorker32B | 38 | 36 | 36 | 1.000 | 36 | 36 | 1.000 | 1.000 | 1.000 | 0.6 | 1.5 | yes |
| mkPktMerge | 46 | 38 | 34 | 0.895 | 34 | 34 | 1.000 | 1.000 | 1.003 | 0.1 | 12.4 | yes |
| mkSMAdapter4B | 50 | 54 | 46 | 0.852 | 58 | 48 | 0.828 | 1.020 | 0.936 | 0.5 | 24.1 | yes |
| or1200 | 92 | 76 | 72 | 0.947 | 88 | 84 | 0.955 | 0.997 | 0.989 | 1.4 | 185.8 | yes |
| raygentop | 58 | 56 | 52 | 0.929 | 62 | 52 | 0.839 | 1.023 | 0.988 | 0.4 | 12.5 | yes |
| sha | 68 | 58 | 54 | 0.931 | 70 | 64 | 0.914 | 1.015 | 0.970 | 0.6 | 7.3 | yes |
| spree | 62 | 62 | 58 | 0.935 | 64 | 58 | 0.906 | 0.965 | 0.974 | 0.2 | 3.0 | yes |
| stereovision0 | 48 | 46 | 44 | 0.957 | 48 | 44 | 0.917 | 1.000 | 0.999 | 2.4 | 72.3 | yes |
| stereovision1 | 80 | 72 | 70 | 0.972 | 74 | 76 | 1.027 | 1.021 | 0.997 | 4.4 | 76.3 | yes |
| stereovision2 | 68 | 62 | 52 | 0.839 | 66 | 56 | 0.848 | 1.052 | 0.943 | 8.1 | 2798.9 | yes |
| stereovision3 | 30 | 30 | 26 | 0.867 | 30 | 26 | 0.867 | 0.971 | 0.994 | 0.0 | 0.2 | yes |
| Geomean |  |  |  | 0.937 |  |  | 0.934 | 1.001 | 0.992 |  |  |  |

stereovision2 took the loop 8 steps, from 62 tracks with `guide`'s router down to 48, in 47
minutes; the search for the narrowest channel then found 52, as a routing that succeeds within
50 iterations at one width can fail at the next one up. Its critical path is 5% longer, with 6%
less wire.

The two largest have seed 1 only. On mcml, at 112 tracks, 2 below the one pass's
114 with `guide`'s router, the best of the four routings left one node overused, after 25 minutes,
so its placement is the one pass's. Placing the whole device again instead, `--whole`, routed
at 112 tracks after a single re-placement, in 13 minutes, the bounding boxes 1.2% smaller and
the critical path by the delay model 1.3% longer; that placement was not measured by VPR, whose search for its
narrowest channel takes hours. On LU32PEEng, the loop routed at 118 tracks, 2 below the one
pass's 120 with `guide`'s router, after two re-placements, in 80 minutes, and went on to 116;
it too was measured by `guide`'s router alone, and is left out of the tables.

#### Seeds

Each pair is the one-pass flow with a seed and the loop from it, as geometric means of the loop
over the one-pass flow, and of each over VPR's placement with seed 1, with VPR's router:

| Suite, seed | Circuits | Min W / one pass, guide | Min W / one pass, VPR | One pass / VPR's placement, VPR | Loop / VPR's placement, VPR | CPD / one pass, VPR | Wire / one pass, VPR | Time, one pass + loop / one pass | All legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| MCNC, seed 1 | 20 | 0.945 | 0.979 | 0.981 | 0.961 | 1.004 | 0.989 | 14.81 | yes |
| MCNC, seed 2 | 20 | 0.954 | 0.994 | 0.975 | 0.970 | 1.000 | 0.982 | 12.33 | yes |
| MCNC, seed 3 | 20 | 0.958 | 0.990 | 0.970 | 0.960 | 1.004 | 1.000 | 10.86 | yes |
| Verilog, seed 1 | 19 | 0.937 | 0.934 | 0.970 | 0.906 | 1.001 | 0.992 | 25.84 | yes |
| Verilog, seed 2 | 19 | 0.943 | 0.952 | 0.971 | 0.925 | 0.999 | 0.995 | 19.31 | yes |
| Verilog, seed 3 | 19 | 0.917 | 0.938 | 0.984 | 0.923 | 0.997 | 0.988 | 21.10 | yes |

With the whole device placed again, `--whole`, instead of windows, seed 1:

| Suite, seed | Circuits | Min W / one pass, guide | Min W / one pass, VPR | One pass / VPR's placement, VPR | Loop / VPR's placement, VPR | CPD / one pass, VPR | Wire / one pass, VPR | Time, one pass + loop / one pass | All legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| MCNC, seed 1, whole | 20 | 0.953 | 0.982 | 0.981 | 0.964 | 1.003 | 0.985 | 14.51 | yes |
| Verilog, seed 1, whole | 19 | 0.921 | 0.931 | 0.970 | 0.903 | 1.007 | 0.984 | 30.13 | yes |

By circuit on the Verilog suite, the narrowest channel with seeds 1, 2 and 3:

| Circuit | VPR's placement, VPR | One pass, VPR | Loop, VPR | One pass, guide | Loop, guide |
| --- | --- | --- | --- | --- | --- |
| LU8PEEng | 92 | 92, 92, 94 | 88, 88, 86 | 86, 88, 84 | 84, 84, 82 |
| arm_core | 104 | 114, 108, 110 | 104, 108, 102 | 104, 108, 108 | 102, 104, 100 |
| bgm | 70 | 68, 70, 66 | 66, 68, 66 | 68, 68, 66 | 66, 66, 64 |
| blob_merge | 58 | 62, 62, 62 | 62, 62, 62 | 62, 60, 62 | 60, 60, 60 |
| boundtop | 42 | 40, 44, 42 | 36, 34, 36 | 40, 44, 42 | 36, 34, 36 |
| ch_intrinsics | 42 | 38, 36, 42 | 36, 36, 34 | 38, 36, 42 | 36, 36, 34 |
| diffeq1 | 60 | 50, 52, 56 | 50, 50, 46 | 46, 48, 50 | 44, 40, 44 |
| diffeq2 | 46 | 40, 44, 48 | 40, 42, 42 | 38, 40, 46 | 38, 42, 38 |
| mkDelayWorker32B | 38 | 36, 36, 38 | 36, 36, 38 | 36, 36, 38 | 36, 36, 38 |
| mkPktMerge | 46 | 34, 40, 42 | 34, 36, 36 | 38, 40, 40 | 34, 36, 36 |
| mkSMAdapter4B | 50 | 58, 56, 56 | 48, 48, 50 | 54, 54, 54 | 46, 46, 48 |
| or1200 | 92 | 88, 90, 90 | 84, 80, 98 | 76, 76, 76 | 72, 70, 70 |
| raygentop | 58 | 62, 52, 52 | 52, 52, 52 | 56, 54, 54 | 52, 52, 52 |
| sha | 68 | 70, 60, 60 | 64, 60, 60 | 58, 56, 56 | 54, 56, 54 |
| spree | 62 | 64, 64, 64 | 58, 60, 58 | 62, 62, 62 | 58, 58, 58 |
| stereovision0 | 48 | 48, 46, 50 | 44, 44, 46 | 46, 44, 50 | 44, 42, 42 |
| stereovision1 | 80 | 74, 74, 72 | 76, 70, 72 | 72, 68, 68 | 70, 66, 68 |
| stereovision2 | 68 | 66, 64, 66 | 56, 66, 66 | 62, 54, 60 | 52, 50, 50 |
| stereovision3 | 30 | 30, 34, 28 | 26, 34, 28 | 30, 34, 28 | 26, 34, 28 |

### What the results say

- **The loop needs narrower channels than the one pass, on average on every seed.** With
  `guide`'s router, which the loop works with, 4.2% to 5.5% narrower on MCNC and 5.7% to 8.3% on
  the Verilog suite. With VPR's router, which never saw the loop, 0.6% to 2.1% narrower on MCNC,
  where seeds move the means by about as much, and 4.8% to 6.6% on the Verilog suite, well
  beyond that.
- **Against VPR's own placement**, with VPR's router, the one-pass flow needed channels 1.9% to
  3.0% narrower on MCNC and 1.6% to 3.0% on the Verilog suite; the loop, 3.0% to 4.0% and 7.5%
  to 9.4%.
- **The circuits that were worst against VPR are no worse than it now, but for one.**
  mkSMAdapter4B, 16% wider than VPR's placement before, now needs 48, 48 and 50 tracks on the
  three seeds, against VPR's 50, from 58, 56 and 56. With seed 1, arm_core went from 114 to 104,
  VPR's own width; raygentop from 62 to 52, against VPR's 58; stereovision2 from 66 to 56,
  against 68; sha from 70 to 64, against 68. blob_merge stays at 62 tracks with VPR's router,
  against VPR's 58, on all three seeds, though `guide`'s router went to 60 on each.
- **At no cost in delay or wire**: critical paths within 0.4% of the one pass's on average, and
  up to 1.8% less wire, with VPR's router at the relaxed width. Single circuits move more:
  s298's critical path is 9% longer and stereovision2's 5%, misex3's 5% shorter.
- **Time.** The loop routes again at every width it tries, so it takes much longer than placing:
  1.4 s to 28 s on MCNC, against 0.1 s to 1.7 s of placement; up to 4.4 min on the Verilog
  suite but for the largest, 14 min on LU8PEEng and 28 to 47 min on stereovision2. Most of it is
  routing at widths that fail, 50 iterations each.

### What the feedback does

Before the runs above, the loop ran once on twelve of the circuits where `guide`'s placement
needed the widest channels against VPR's, arm_core, blob_merge, mkSMAdapter4B, or1200, raygentop,
sha, spree, ex1010, pdc, s298, seq and spla, from the one-pass placement with seed 1, at 2 and at
4 tracks below its narrowest channel with `guide`'s router, with up to 3 placements again unless
said. Counted are the circuits whose loop ended with a legal routing:

| Setting | 2 tracks narrower | 4 tracks narrower | Time, s, 2 narrower |
| --- | --- | --- | --- |
| **Windows of 6 tiles, history and use** (the default) | **12** | **9** | **110** |
| Windows of 4 tiles | 10 | 8 | 222 |
| Windows of 8 tiles | 12 | 11 | 154 |
| The whole device | 12 | 12 | 122 |
| The whole device, 6 times | 12 | 12 | 124 |
| Windows of 6 tiles, routing from the routes before (`--seed-routes`) | 12 | 10 | 118 |
| Windows of 4 tiles, congestion factor 2 | 11 |  | 194 |
| Windows of 4 tiles, without feedback | 8 |  | 235 |
| The whole device, without feedback | 9 |  | 171 |
| The whole device, without feedback, 6 times |  | 5 |  |
| The whole device, history only | 12 |  | 147 |
| The whole device, use beyond the estimate only | 8 |  | 194 |

- **The router's feedback is what does it.** Placing again at the same low temperature with
  only the placer's own estimate of congestion, the loop narrowed 8 and 9 of the 12 circuits by 2
  tracks and 5 by 4, against 12 and 9 to 12 with it. Placing again is not just more annealing.
- **History does nearly all of it.** With the router's history of overuse alone, all 12 went 2
  narrower; with only how much more of each channel the routes used than RUDY expected, 8.
- **Windows or the whole device.** Windows of 6 tiles around what was overused cover 91% of an
  MCNC device on average and 68% of a Verilog one, and did as well as the whole device 2 tracks
  narrower, and less well 4 narrower, which the loop never asks for at once. The default keeps
  the re-placement local, as the roadmap asks; `--whole` is there.
- **Routing again from the routes before**, through the engine's `Engine::seed_routes`, routed as
  many circuits in about as long, so it stays off. The router's history goes into the placement,
  and each routing starts afresh from it.

### Pin permutation

What the netlist lets a router permute on these architectures, and what each flow does with it:

| Where | `k6_N10_40nm.xml` (MCNC) | `k6_frac_N10_frac_chain_mem32K_40nm.xml` (Verilog) | VPR 9.0.0 | `guide` |
| --- | --- | --- | --- | --- |
| A cluster's inputs | 40, `equivalent="full"` | 40, `equivalent="full"` | One `SINK` for the class, any pin; the netlist is rewritten to match after routing (`.net.post_routing`) | The same `SINK`, any pin, already before this |
| A cluster's outputs | 10, `equivalent="instance"` | 20, `equivalent="none"` | One `SOURCE`, one pin per net | The same, already: `fpga::Design` roots a net on one of the `OPIN`s |
| A LUT's inputs, inside the cluster | 6, from a `complete` crossbar, 82 ps to 398 ps | 6 or 5, from a `complete` crossbar, all 261 ps or all 235 ps | As the packer left them, for routability inside the cluster | **Rotated for timing after routing**, below |
| Memories' address and data bits, multipliers' operands | Not in MCNC | `none` | Not permuted | Not permuted |

So between clusters both routers already exploit all the equivalence the architectures declare.
Inside a cluster, a LUT's inputs come from a crossbar that reaches every one of them, so which
input a signal takes changes nothing that either router sees, and `.net` files record the
choice in a `port_rotation_map`, which VPR's timing analysis reads. On `k6_N10_40nm.xml` the
choice matters: the six inputs take 82, 173, 261, 263, 398 and 397 ps. VPR's packer chooses
inputs for routability inside the cluster, and nothing in VPR's flow changes them for timing.

`vtr_route --rotate-luts` does, after a legal routing: with every connection's routed delay in
the timing graph, each LUT's latest-arriving signals go to its fastest inputs, until no LUT
changes (`fpga::rotate_luts_for_timing`), and `fpga::write_rotated_netlist` writes the `.net`
with the signals and rotation maps moved. VPR then reads the rotated netlist, with the same
placement and routes, and checks and times them with `--analysis`; and its router routes the
placement again with the rotated netlist, at the same relaxed width. Applied to VPR's own
placement too, to show that it is independent of the placer:

| Circuit | Placement | LUTs rotated | guide's routes, ns | Rotated, ns | / not | VPR's routes, ns | Rotated, ns | / not | Wire / not, VPR | Legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| alu4 | vpr | 905 of 912 | 4.8465 | 3.894 | 0.803 | 5.31783 | 4.04005 | 0.760 | 0.974 | yes |
| alu4 | guide | 904 of 912 | 4.99054 | 3.85597 | 0.773 | 4.99275 | 4.08568 | 0.818 | 1.034 | yes |
| apex2 | vpr | 1065 of 1075 | 5.77742 | 4.30064 | 0.744 | 5.80687 | 4.50526 | 0.776 | 1.014 | yes |
| apex2 | guide | 1067 of 1075 | 5.59581 | 4.21745 | 0.754 | 5.76134 | 4.65385 | 0.808 | 1.029 | yes |
| apex4 | vpr | 881 of 887 | 5.27845 | 4.18199 | 0.792 | 5.38635 | 4.31389 | 0.801 | 0.969 | yes |
| apex4 | guide | 882 of 887 | 5.27103 | 4.17239 | 0.792 | 5.34477 | 4.31704 | 0.808 | 1.034 | yes |
| bigkey | vpr | 894 of 911 | 3.2653 | 2.51122 | 0.769 | 3.30692 | 2.6085 | 0.789 | 1.039 | yes |
| bigkey | guide | 893 of 911 | 3.25733 | 2.55301 | 0.784 | 3.2681 | 2.57789 | 0.789 | 1.002 | yes |
| clma | vpr | 3554 of 3576 | 8.66512 | 6.49691 | 0.750 | 8.69463 | 6.61225 | 0.760 | 1.020 | yes |
| clma | guide | 3556 of 3576 | 8.4878 | 6.31266 | 0.744 | 8.47252 | 6.56294 | 0.775 | 1.020 | yes |
| des | vpr | 633 of 698 | 4.46377 | 3.37855 | 0.757 | 4.46945 | 3.37981 | 0.756 | 0.996 | yes |
| des | guide | 633 of 698 | 4.43359 | 3.2217 | 0.727 | 4.41387 | 3.3353 | 0.756 | 1.009 | yes |
| diffeq | vpr | 845 of 930 | 6.10312 | 4.43269 | 0.726 | 6.09481 | 4.55041 | 0.747 | 0.991 | yes |
| diffeq | guide | 846 of 930 | 5.79765 | 4.29822 | 0.741 | 6.12015 | 4.69225 | 0.767 | 1.010 | yes |
| dsip | vpr | 873 of 909 | 3.17582 | 2.52701 | 0.796 | 3.17889 | 2.55214 | 0.803 | 0.985 | yes |
| dsip | guide | 873 of 909 | 3.4161 | 2.58475 | 0.757 | 3.43943 | 2.57915 | 0.750 | 1.047 | yes |
| elliptic | vpr | 2063 of 2168 | 7.54636 | 5.49148 | 0.728 | 7.58148 | 5.72609 | 0.755 | 1.021 | yes |
| elliptic | guide | 2062 of 2168 | 7.51422 | 5.93823 | 0.790 | 7.51865 | 6.21948 | 0.827 | 1.028 | yes |
| ex1010 | vpr | 2631 of 2649 | 6.82146 | 5.82022 | 0.853 | 6.89706 | 5.99618 | 0.869 | 1.022 | yes |
| ex1010 | guide | 2640 of 2649 | 6.63487 | 5.55452 | 0.837 | 6.65474 | 5.72121 | 0.860 | 1.027 | yes |
| ex5p | vpr | 741 of 753 | 4.54943 | 3.64246 | 0.801 | 4.62135 | 3.80697 | 0.824 | 1.046 | yes |
| ex5p | guide | 741 of 753 | 4.34903 | 3.41984 | 0.786 | 4.56112 | 3.77477 | 0.828 | 0.997 | yes |
| frisc | vpr | 2075 of 2269 | 9.59249 | 7.12923 | 0.743 | 10.0072 | 7.38275 | 0.738 | 1.015 | yes |
| frisc | guide | 2078 of 2269 | 9.66988 | 7.24628 | 0.749 | 9.74947 | 7.24233 | 0.743 | 0.998 | yes |
| misex3 | vpr | 803 of 814 | 4.78367 | 3.71028 | 0.776 | 4.82071 | 3.85063 | 0.799 | 1.034 | yes |
| misex3 | guide | 806 of 814 | 4.89592 | 3.64127 | 0.744 | 5.06325 | 3.7956 | 0.750 | 1.006 | yes |
| pdc | vpr | 2804 of 2823 | 7.05184 | 5.49903 | 0.780 | 7.06044 | 6.02164 | 0.853 | 1.029 | yes |
| pdc | guide | 2807 of 2823 | 7.02664 | 5.46673 | 0.778 | 7.52518 | 6.46751 | 0.859 | 1.043 | yes |
| s298 | vpr | 709 of 714 | 7.35173 | 5.11921 | 0.696 | 7.26292 | 5.47105 | 0.753 | 1.041 | yes |
| s298 | guide | 709 of 714 | 6.64345 | 4.83847 | 0.728 | 6.64124 | 5.23877 | 0.789 | 0.996 | yes |
| s38417 | vpr | 2992 of 3290 | 5.51245 | 4.15302 | 0.753 | 5.80047 | 4.34468 | 0.749 | 1.013 | yes |
| s38417 | guide | 2994 of 3290 | 5.30108 | 3.90559 | 0.737 | 5.49631 | 4.05331 | 0.737 | 0.999 | yes |
| s38584.1 | vpr | 2843 of 3123 | 4.87983 | 4.02034 | 0.824 | 5.05609 | 4.61144 | 0.912 | 0.984 | yes |
| s38584.1 | guide | 2845 of 3123 | 5.21921 | 4.17104 | 0.799 | 5.2779 | 4.62522 | 0.876 | 1.004 | yes |
| seq | vpr | 952 of 965 | 4.794 | 3.7739 | 0.787 | 4.87201 | 3.89722 | 0.800 | 1.006 | yes |
| seq | guide | 949 of 965 | 4.88102 | 3.85624 | 0.790 | 5.00909 | 3.9622 | 0.791 | 1.022 | yes |
| spla | vpr | 2192 of 2216 | 6.51736 | 5.13829 | 0.788 | 6.52939 | 5.56467 | 0.852 | 1.036 | yes |
| spla | guide | 2182 of 2216 | 6.21489 | 4.88286 | 0.786 | 6.29578 | 5.40838 | 0.859 | 1.045 | yes |
| tseng | vpr | 896 of 1027 | 5.68422 | 3.93343 | 0.692 | 5.71256 | 4.17108 | 0.730 | 1.025 | yes |
| tseng | guide | 896 of 1027 | 5.61643 | 3.90923 | 0.696 | 5.64603 | 4.02116 | 0.712 | 0.995 | yes |
| Geomean | vpr |  |  |  | 0.767 |  |  | 0.790 | 1.013 |  |
| Geomean | guide |  |  |  | 0.764 |  |  | 0.794 | 1.017 |  |

- **Critical paths are 24% shorter** by VPR's analysis of `guide`'s routes, 0.764 of what they
  were on `guide`'s placements and 0.767 on VPR's, and 21% shorter with VPR's router, every
  route legal. `guide`'s estimate before writing the netlist was within 4 ps of VPR's analysis
  after it on `tseng` (3.905 ns against 3.909 ns). The wire VPR's router uses is 1.3% to
  1.7% more, as its timing-driven router weighs nets differently with the new criticalities.
- It composes with the loop: on the loop's seed-1 placements, rotation takes `guide`'s routes to
  0.762 of their critical path, and VPR's to 0.782.
- On the Verilog architecture every input of a LUT is as fast, so there is nothing to gain:
  on or1200, all 681 LUTs can be rotated and none is, and VPR times the netlist as written at
  8.841 ns, as before.
- Rotation takes 0.03 s to 0.16 s a circuit.
- **Not done**: permuting which fracturable LE of a cluster implements which function, which
  would let a net leave by another output pin of a Verilog cluster, whose outputs are
  `equivalent="none"`; it would mean rewriting the cluster's internal routing, around its carry
  chains. Nor are memories' address bits, which a memory without initial contents allows if
  every port is permuted alike, nor multipliers' operands.

### What it took

- **The router's history, more than its last iteration.** Feeding back only how much more of
  each channel the routes used than RUDY expected, the use at the end, narrowed 8 of the 12
  circuits of the table above; the history of overuse, added up over every iteration and kept
  over rounds, as the router's own history is, narrowed all 12.
- **Placing again at a low temperature.** Annealing from where blocks are, at a twentieth of
  the moves' standard deviation over 4 steps, with timing, keeps what the one pass found: VPR's
  estimate of the wire before routing came to 0.989 and 0.997 of the one pass's on the two
  suites with seed 1, between 0.95 and 1.04 on single circuits, and critical paths stayed within
  0.4%.
- **Nothing new in the engine.** The history comes from `Engine::overused` after each step, and
  the windows from where the best iteration's overused nodes are, so the loop is all in
  `src/fpga`. Routing again from the routes before, through the engine's `seed_routes`, which
  came in parallel, is an option.
- **A packed netlist written back.** For the rotated `.net` to be the same packing, every level
  between the crossbar and the LUT has to move a signal together, and the `port_rotation_map`
  with them; `fpga::rotatable_luts` checks that each level takes its pins straight from its
  parent, which nothing else in the parent uses, from a `complete` crossbar of the architecture.
  VPR rewrites a placement file whose header names its netlist otherwise than as given on its
  command line, which changes the digest a route file names, so `rotate-circuit.sh` writes the
  header as VPR would.

### What this does not show

- LU32PEEng and mcml: their loops were measured by `guide`'s router alone, as VPR's search for
  their narrowest channel takes hours.
- One packing per circuit, VPR's with seed 1, and the loop works at the widths of `guide`'s
  router. A loop that targets VPR's router would route with it, as it is the router's own
  history that the placement takes.
- Times are of a shared machine, busy with other runs, and are only good to an order of
  magnitude.
- LUT rotation is measured on MCNC alone, as the Verilog architecture's LUT inputs are all as
  fast. Architectures whose clusters have sparse crossbars would make rotation a matter of
  routing inside the cluster too, which neither flow here does.

## Routing reticle's designs

[reticle](https://github.com/KarpelesLab/reticle) is expected to adopt `guide`
([target consumers](architecture.md#target-consumers)), so `guide` has to route reticle's own
designs on reticle's own devices at least as well as reticle's router does. This compares the
two on every design reticle builds a bitstream for with its own place and route, on the
same placements and routing graphs, with every result of both checked by reticle itself.

Measured on 2026-11-17 with rustc 1.97.1, on an AMD Ryzen Threadripper PRO 9995WX shared with
other work, every run under `nice -n 19` and four designs at a time; `guide` at `ad406db` and
reticle at `1b20721c`. The times are noisy for that: the same route measured at different
loads differed by up to a factor of two. Both routers of a design run one after the other in
one process, on one placement, so their times were taken at about the same load.

### How

- **The flow** is reticle's own, up to and after routing, as `reticle fpga --bitstream` runs
  it with no other option, transcribed from `src/bin/reticle/main.rs` into
  [`scripts/reticle/route`](../scripts/reticle/route), a crate of its own with path
  dependencies on `guide` and on reticle's checkout beside it: synthesis, mapping and packing,
  the device database's fabric (on the 7-series, the region reticle loads around the
  constrained pins), placement by reticle with its default seed, and the routing options
  reticle's flow builds for the family. One difference: reticle places a design again with
  the next seed when its route does not converge (`--route-attempts`); the harness keeps the
  one placement, since both routers are compared on it.
- **The devices** are those reticle has a database for that it fetches itself (`reticle
  fetch`, into `~/.cache/reticle`): the Basys 3's Xilinx XC7A35T by Project X-Ray, the
  Cynthion's Lattice LFE5U-12F by Project Trellis and the Tang Primer 20K's Gowin GW2A-18 by
  Project Apicula. Not the ZCU104's UltraScale+: reticle routes it inside its library's
  `uray::zcu104::implement`, which also needs the PYNQ base overlay's bitstream to borrow its
  clock network (`--borrow-clock`), and neither is to hand. Not the iCE40: reticle's one
  built-in iCE40 architecture is the HX1K's, and the examples that target an iCE40 are for
  the HX8K. No database is copied here.
- **The designs** are the 43 of [`designs.txt`](../scripts/reticle/route/designs.txt): every
  Basys 3 example with its own constraints, the SoC, NES and Apple II on the Basys 3, every
  Cynthion design in reticle's `testdata/fpga/cynthion` with constraints, the 6502 monitor on
  the Cynthion, and the Primer 20K examples. The ChipWhisperer-Husky, reticle's largest, needs
  NewAE's sources, which are not in reticle's repository. The largest here are a USB crypto
  console of 13 342 signals on the ECP5 and an NES of 5342 on the XC7A35T, over graphs of 1.1
  and 2.9 to 5.7 million nodes.
- **`guide`'s graph** is reticle's `RoutingGraph` seen as a `RoutingSpace` whose nodes are
  reticle's *physical* nodes (`route::physical_nodes`): a 7-series wire has a name in every
  tile it crosses, joined to the next by a bit-less pip each way, and reticle keeps capacity per
  piece of metal and refuses a route that drives one twice. A tree over physical nodes cannot
  do either. Each net is held to what reticle holds its signal to: closed nodes, the clock
  network open only to clocks, a clock entering its pin's wire only from the network, a clock
  reaching its pins over the network alone, and a sink's only way in reserved for it at
  reticle's price. A lookup table's inputs that reticle lets the router permute
  (`RouteOptions::equivalent`) are one terminal with each input an access point. The lower
  bound is reticle's own estimate, its `astar_weight` per tile of distance scaled by the node's
  base cost, and the negotiation's factors are reticle's for the family; besides those,
  `guide` routes with incremental rerouting and `EngineOptions::target_terminals` at 1 (below).
- **Back to reticle.** Reticle's `Routing` cannot be built from routes through its public API,
  and everything after routing takes one. So `guide`'s routes are replayed: reticle's own
  router is run again on a copy of the graph in which every pip `guide` did not choose reads
  and drives the same node, but for the joins inside a physical node, with every node `guide`
  did not use closed. What comes back is reticle's own `Routing`, held to all of its rules,
  with the real graph's pip numbers. **All 100 626 signals of the 42 designs that have any came
  back with exactly `guide`'s pips.**
- **Legal** means that a result passed everything reticle does before it writes a bitstream:
  `Routing::verify` (each sink walked back to its driver), `Routing::shorts`, the clocks on the
  global network, the cells configured, and the bitstream decoded back through the database,
  with no set bit unexplained and the arcs the bits select exactly the arcs routed (the
  7-series also checks each lookup table decodes as placed and routed). The Gowin database
  decodes nothing back, so there it is `verify`, `shorts` and the bitstream's tiles generating,
  which is more than reticle's own flow checks there.
- **Programmable pips** are the pips with configuration bits in the final `Routing`, the same
  count for both: a 7-series route also takes a join for every tile a wire crosses, which costs
  nothing. **Latest path** is reticle's own timing of the final routing: on the 7-series the
  slowest clock domain's latest path by the model `--timing-driven` routes by
  (`place::path_criticality` on `route::connection_delays`, Vivado-fitted delays and branch
  loads); reticle has no delay model for the ECP5 or Gowin, so there it is the longest
  register-to-register path counted in graph nodes (`route::latest_paths`, one unit a node).
- **Time** is the routing call alone, for `guide` including building its physical graph from
  reticle's (up to 0.9 s on the 7-series graphs, 0.3 s on the ECP5's) but not the replay.
  **Memory** is the peak resident set during routing over what was resident before it, with
  the peak reset through `/proc/self/clear_refs`.

```text
# reticle's checkout beside guide's, and its databases fetched:
reticle fetch prjxray-db && reticle fetch prjtrellis-db && reticle fetch apicula
JOBS=4 scripts/reticle/route/run.sh OUT              # all 43 designs, about an hour
python3 scripts/reticle/route/tables.py OUT          # the table below
ROUTERS=guide,guide@8 JOBS=1 scripts/reticle/route/run.sh OUT2 nes_basys3 ...  # threads
ARGS=--timing-driven ROUTERS=reticle,guide,guide:timed scripts/reticle/route/run.sh OUT3 ...
```

### Results

**Every design that reticle's router routes legally, `guide` routes legally too**, and the
three that fail fail the same way for both, on the same placement:

- `spi_console`: reticle's placement leaves a carry chain's sum with no wire at all to the
  flip-flop it feeds (`no path exists`), which no router can mend.
- `nanos_seph_usb`: one set bit of the bitstream belongs to no feature of Project Trellis'
  database, with either router's routes.
- `wide_state`: three flip-flops' clocks reach them off the global network, with either.

Over the 31 designs of 100 signals or more that both route, by geometric mean, `guide` takes
**0.77 times reticle's time**, uses **0.90 times the programmable pips**, and gives **latest
paths 0.96 times as long**, with 1.4 times the memory. By family:

| Designs of 100 signals or more | Designs | Time | Programmable pips | Latest path | Memory |
| --- | ---: | ---: | ---: | ---: | ---: |
| XC7A35T, all | 15 | 1.26 | 0.83 | 0.90 | 0.98 |
| XC7A35T, 1000 signals or more | 9 | 0.99 | 0.87 | 0.85 | 1.00 |
| LFE5U-12F, all | 16 | 0.48 | 0.98 | 1.01 | 1.96 |
| LFE5U-12F, 1000 signals or more | 12 | 0.42 | 0.98 | 1.00 | 1.97 |

| Design | Device | Signals | Graph nodes | Legal (reticle / guide) | Time, s (reticle / guide / guide@8) | Iterations | Programmable pips | Latest path | Peak memory, MiB |
| --- | --- | ---: | ---: | --- | --- | --- | --- | --- | --- |
| iso_display | xc7a35t-cpg236 | 7083 | 5721505 | yes / yes | 3.16 / 3.12 / - | 10 / 9 | 43353 / 38430 | 16.28 / 13.47 ns | 628 / 614 |
| nes_basys3 | xc7a35t-cpg236 | 5342 | 2941591 | yes / yes | 16.42 / 9.98 / - | 16 / 17 | 70436 / 59485 | 49.30 / 48.10 ns | 324 / 305 |
| iso7816_terminal | xc7a35t-cpg236 | 5183 | 5721505 | yes / yes | 2.46 / 2.29 / - | 10 / 9 | 32075 / 28822 | 15.11 / 12.95 ns | 591 / 668 |
| apple2_basys3 | xc7a35t-cpg236 | 2605 | 5353485 | yes / yes | 5.00 / 3.91 / - | 10 / 9 | 29767 / 27490 | 36.12 / 31.82 ns | 565 / 580 |
| soc_basys3 | xc7a35t-cpg236 | 2508 | 5353485 | yes / yes | 2.79 / 2.89 / - | 11 / 11 | 24337 / 21546 | 21.02 / 23.46 ns | 591 / 612 |
| ssd1306_console | xc7a35t-cpg236 | 2095 | 5721505 | yes / yes | 1.20 / 1.32 / - | 8 / 7 | 12703 / 10915 | 15.22 / 8.74 ns | 620 / 614 |
| iso7816_probe | xc7a35t-cpg236 | 1834 | 5475395 | yes / yes | 0.99 / 1.31 / - | 9 / 9 | 10594 / 9377 | 9.85 / 9.13 ns | 603 / 553 |
| selftest | xc7a35t-cpg236 | 1448 | 5706490 | yes / yes | 1.07 / 1.27 / - | 8 / 7 | 10851 / 8886 | 16.36 / 11.72 ns | 657 / 617 |
| spi_console | xc7a35t-cpg236 | 1285 | 5475395 | **no** / **no** | - / - / - | - / - | - | - | - |
| partsel | xc7a35t-cpg236 | 1220 | 5475395 | yes / yes | 1.38 / 1.59 / - | 11 / 10 | 9949 / 8670 | 15.12 / 13.82 ns | 562 / 569 |
| carry_probe | xc7a35t-cpg236 | 195 | 3294596 | yes / yes | 0.32 / 0.61 / - | 8 / 8 | 1075 / 922 | 5.83 / 5.79 ns | 363 / 353 |
| carry_where | xc7a35t-cpg236 | 195 | 3396477 | yes / yes | 0.33 / 0.53 / - | 5 / 5 | 1524 / 1299 | 5.82 / 4.56 ns | 374 / 341 |
| io_exercise | xc7a35t-cpg236 | 167 | 5375600 | yes / yes | 0.49 / 0.87 / - | 6 / 7 | 1848 / 1366 | 3.20 / 3.14 ns | 602 / 619 |
| pll_blink | xc7a35t-cpg236 | 165 | 3294596 | yes / yes | 0.29 / 0.56 / - | 6 / 8 | 1045 / 963 | 3.19 / 3.98 ns | 366 / 355 |
| lutram | xc7a35t-cpg236 | 106 | 3294596 | yes / yes | 0.32 / 0.55 / - | 6 / 7 | 990 / 722 | 3.35 / 3.24 ns | 366 / 336 |
| blink_carry | xc7a35t-cpg236 | 105 | 5375600 | yes / yes | 0.43 / 0.85 / - | 2 / 7 | 593 / 319 | 2.66 / 2.66 ns | 599 / 602 |
| blink | xc7a35t-cpg236 | 86 | 3294596 | yes / yes | 0.29 / 0.60 / - | 8 / 7 | 654 / 580 | 3.67 / 3.59 ns | 361 / 344 |
| bram_rom | xc7a35t-cpg236 | 17 | 4125371 | yes / yes | 0.34 / 0.74 / - | 2 / 3 | 387 / 217 | - | 445 / 451 |
| pmod_bidir | xc7a35t-cpg236 | 4 | 234835 | yes / yes | 0.01 / 0.03 / - | 1 / 1 | 70 / 38 | - | 14 / 9 |
| sw_led | xc7a35t-cpg236 | 3 | 103117 | yes / yes | 0.00 / 0.01 / - | 1 / 2 | 23 / 14 | - | 0 / 6 |
| usb_crypto_console | ecp5-12f-CABGA256 | 13342 | 1096425 | yes / yes | 189.78 / 100.04 / - | 5 / 6 | 101742 / 99226 | 209 / 209 units | 135 / 253 |
| mos6502_monitor | ecp5-12f-CABGA256 | 9629 | 1096425 | yes / yes | 192.15 / 69.40 / - | 6 / 6 | 83549 / 81866 | 271 / 305 units | 134 / 259 |
| usb_mjpeg | ecp5-12f-CABGA256 | 8203 | 1096425 | yes / yes | 118.67 / 44.83 / - | 5 / 6 | 56052 / 55102 | 197 / 198 units | 118 / 255 |
| usb_inflate | ecp5-12f-CABGA256 | 4972 | 1096425 | yes / yes | 76.92 / 34.80 / - | 5 / 6 | 37378 / 36735 | 137 / 137 units | 133 / 261 |
| usb_host_target | ecp5-12f-CABGA256 | 4899 | 1096425 | yes / yes | 90.62 / 31.67 / - | 6 / 7 | 34140 / 33625 | 89 / 92 units | 131 / 255 |
| nanos_seph_usb | ecp5-12f-CABGA256 | 4882 | 1096425 | **no** / **no** | 81.76 / 36.51 / - | 5 / 6 | 36066 / 35656 | 65 / 70 units | 128 / 258 |
| smartcard_serial | ecp5-12f-CABGA256 | 4810 | 1096425 | yes / yes | 90.46 / 34.13 / - | 5 / 5 | 38018 / 37127 | 113 / 119 units | 132 / 262 |
| usb_proxy_target | ecp5-12f-CABGA256 | 4807 | 1096425 | yes / yes | 82.74 / 35.81 / - | 5 / 6 | 35230 / 34634 | 61 / 61 units | 133 / 263 |
| usb_hub_target | ecp5-12f-CABGA256 | 3730 | 1096425 | yes / yes | 59.54 / 25.62 / - | 5 / 5 | 25794 / 25364 | 67 / 61 units | 114 / 253 |
| usb_ulpi_trace | ecp5-12f-CABGA256 | 3485 | 1096425 | yes / yes | 123.39 / 40.38 / - | 5 / 5 | 20952 / 20450 | 65 / 64 units | 133 / 259 |
| usb_cdc_uart | ecp5-12f-CABGA256 | 3031 | 1096425 | yes / yes | 51.20 / 21.83 / - | 5 / 6 | 21368 / 21157 | 122 / 117 units | 137 / 251 |
| usb_bram | ecp5-12f-CABGA256 | 2404 | 1096425 | yes / yes | 31.16 / 16.15 / - | 5 / 5 | 18025 / 17699 | 66 / 63 units | 136 / 261 |
| usb_ulpi_device | ecp5-12f-CABGA256 | 1594 | 1096425 | yes / yes | 21.16 / 11.44 / - | 4 / 5 | 11185 / 11091 | 55 / 54 units | 134 / 257 |
| bidir_bus | ecp5-12f-CABGA256 | 146 | 1096425 | yes / yes | 2.01 / 0.94 / - | 4 / 4 | 929 / 930 | 33 / 29 units | 136 / 259 |
| target_ulpi_loopback | ecp5-12f-CABGA256 | 112 | 1096425 | yes / yes | 0.87 / 0.81 / - | 3 / 3 | 693 / 680 | 26 / 28 units | 135 / 263 |
| bidir_loopback | ecp5-12f-CABGA256 | 102 | 1096425 | yes / yes | 1.48 / 1.01 / - | 4 / 5 | 646 / 628 | 26 / 29 units | 135 / 255 |
| clock_blink | ecp5-12f-CABGA256 | 100 | 1096425 | yes / yes | 1.07 / 0.88 / - | 3 / 5 | 613 / 601 | 25 / 27 units | 137 / 259 |
| wide_state | ecp5-12f-CABGA256 | 9 | 1096425 | **no** / **no** | 0.12 / 0.25 / - | 2 / 2 | 41 / 40 | 16 / 15 units | 134 / 273 |
| button_led | ecp5-12f-CABGA256 | 2 | 1096425 | yes / yes | 0.13 / 0.25 / - | 1 / 1 | 15 / 16 | 20 / 18 units | 133 / 271 |
| leds | ecp5-12f-CABGA256 | 0 | 1096425 | yes / yes | 0.11 / 0.23 / - | 1 / 1 | 0 / 0 | 0 / 0 units | 137 / 275 |
| quiesce | ecp5-12f-CABGA256 | 0 | 1096425 | yes / yes | 0.12 / 0.23 / - | 1 / 1 | 0 / 0 | 0 / 0 units | 134 / 273 |
| buttons | gw2a-18-pg256 | 4 | 626001 | yes / yes | 0.13 / 0.25 / - | 1 / 1 | 51 / 51 | 16 / 16 units | 98 / 243 |
| key_led | gw2a-18-pg256 | 2 | 93370 | yes / yes | 0.01 / 0.02 / - | 1 / 1 | 11 / 11 | 11 / 11 units | 0 / 0 |

On the 7-series, `guide`'s routes use 13% to 17% fewer programmable pips and their latest
paths are 10% to 15% shorter, by reticle's own timing model, with neither router timing-driven;
`soc_basys3` and `pll_blink` are the two whose latest path is longer. Its time is about
reticle's on the larger designs and up to twice reticle's on the small ones, where building its
physical graph from reticle's, 0.5 to 0.9 s, is most of it: a caller routing one graph several times
would build it once. On the ECP5, `guide` takes 0.42 times reticle's time on the larger designs, with 2% fewer
pips and the same latest paths in nodes by geometric mean, though from 0.88 to 1.13 times as
long design by design (`mos6502_monitor` the longest). Its memory there is twice reticle's, its physical
graph of the die's 8.3 million pips, which on the 7-series is in place of about as much of
reticle's own.

### Threads

`guide@8` is `guide` on 8 threads, with batches of 32 nets, which route differently from one
net at a time but the same for any number of threads; each was run on its own, one design at a
time, against `guide` on one thread in the same process.

| Design | Device | Time, s (1 thread / 8) | Speed-up | Programmable pips (1 / 8) | Latest path (1 / 8) | Memory, MiB (1 / 8) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| iso_display | XC7A35T | 2.94 / 1.84 | 1.60 | 38430 / 38570 | 13.47 / 13.27 ns | 610 / 1108 |
| nes_basys3 | XC7A35T | 10.21 / 4.67 | 2.19 | 59485 / 59385 | 48.10 / 46.52 ns | 326 / 651 |
| apple2_basys3 | XC7A35T | 3.32 / 2.45 | 1.35 | 27490 / 27481 | 31.82 / 32.57 ns | 599 / 940 |
| usb_crypto_console | LFE5U-12F | 89.26 / 69.05 | 1.29 | 99226 / 99429 | 209 / 215 units | 260 / 647 |
| mos6502_monitor | LFE5U-12F | 72.07 / 59.79 | 1.21 | 81866 / 81598 | 305 / 296 units | 264 / 688 |
| usb_mjpeg | LFE5U-12F | 42.29 / 33.80 | 1.25 | 55102 / 55130 | 198 / 200 units | 247 / 686 |

Threads help the 7-series more than the ECP5, where batches took 8% to 14% more expansions
than one net at a time and gained only a fifth to a quarter. What holds the ECP5 back was not
measured further.

### Timing-driven routing

With `--timing-driven`, reticle places and routes the 7-series by its timing model: each pass
analysed per clock domain, a critical connection weighing delay over congestion, the lookup
tables' fast inputs claimed, clean-up passes and a refinement after the route is legal. The
harness places that way and gives reticle's router the same `route::Timing` the command line
builds (it does not give a placement whose cells moved after the route another route, as the
command line's `refine_bels` does). `guide` is shown routing for wire, as above, and routing
by reticle's model (`guide:timed`), with reticle's settings for it (below).

Measured on 2026-11-17 as above, every XC7A35T design, four at a time, with `guide` at
`0b91aa3`; reticle has a fitted delay model for no other family. The timing-driven
placement differs from the default one, and on it `iso_display`, `selftest` and `spi_console`
have a carry chain's sum with no wire to the flip-flop it feeds (`no path exists`), for both
routers; four designs have no timed path. That leaves 14. The comparison without timing
above was run again at that commit, all 43 designs: every one came out as in the table
above, legal or not alike, with the same pips and iterations for both routers.

| Design | Signals | Time, s (reticle / guide / guide:timed) | Iterations | Programmable pips | Latest path, ns (reticle / guide / guide:timed) |
| --- | ---: | --- | --- | --- | --- |
| nes_basys3 | 5342 | 38.61 / 4.99 / 22.82 | 34 / 13 / 17 | 47916 / 46270 / 44838 | 16.90 / 26.65 / 17.30 |
| iso7816_terminal | 5183 | 8.84 / 2.51 / 4.82 | 27 / 8 / 15 | 31632 / 29381 / 28525 | 8.14 / 11.53 / 8.06 |
| apple2_basys3 | 2605 | 26.00 / 3.87 / 19.29 | 35 / 9 / 17 | 29248 / 28088 / 26937 | 19.74 / 27.67 / 19.24 |
| soc_basys3 | 2508 | 20.18 / 2.73 / 11.75 | 23 / 9 / 15 | 21672 / 21133 / 20081 | 12.27 / 18.21 / 12.23 |
| ssd1306_console | 2095 | 3.00 / 1.45 / 2.68 | 18 / 8 / 14 | 12581 / 11323 / 11099 | 6.86 / 8.12 / 6.95 |
| iso7816_probe | 1834 | 3.32 / 1.21 / 3.49 | 14 / 8 / 17 | 9722 / 8810 / 8722 | 4.77 / 7.19 / 4.78 |
| partsel | 1220 | 9.41 / 1.68 / 11.17 | 28 / 9 / 18 | 9415 / 8813 / 8562 | 8.33 / 13.87 / 8.52 |
| carry_probe | 195 | 1.11 / 0.58 / 1.33 | 29 / 9 / 13 | 1099 / 965 / 940 | 4.33 / 5.34 / 4.37 |
| carry_where | 195 | 1.25 / 0.55 / 1.14 | 32 / 7 / 16 | 1162 / 967 / 980 | 2.98 / 3.56 / 3.27 |
| io_exercise | 167 | 1.47 / 0.85 / 1.65 | 24 / 7 / 16 | 1872 / 1366 / 1380 | 2.44 / 3.14 / 2.73 |
| pll_blink | 165 | 2.10 / 0.55 / 2.70 | 24 / 6 / 16 | 1051 / 946 / 1003 | 2.36 / 3.21 / 2.60 |
| lutram | 106 | 1.67 / 0.54 / 1.65 | 30 / 6 / 16 | 965 / 694 / 726 | 2.42 / 3.03 / 2.61 |
| blink_carry | 105 | 0.44 / 0.84 / 1.06 | 6 / 7 / 7 | 554 / 319 / 320 | 2.53 / 2.66 / 2.53 |
| blink | 86 | 1.07 / 0.59 / 1.39 | 20 / 8 / 13 | 543 / 510 / 535 | 2.43 / 2.95 / 2.56 |

| By geometric mean, over reticle's | Designs | Time | Programmable pips | Latest path |
| --- | ---: | ---: | ---: | ---: |
| guide:timed, all | 14 | 0.98 | 0.86 | 1.03 |
| guide:timed, 1000 signals or more | 7 | 0.77 | 0.91 | 1.00 |
| guide:timed, under 1000 signals | 7 | 1.25 | 0.80 | 1.07 |
| guide, for wire alone, all | 14 | 0.34 | 0.86 | 1.34 |

All 51 results of the 17 designs routed are legal by reticle's checks, down to the decoded
bitstream, and every one of `guide:timed`'s 21 830 signals came back from the replay with
exactly its pips. **On the seven
designs of 1000 signals or more, `guide` routing by reticle's model matches reticle's
timing-driven router**: latest paths from 0.975 to 1.023 times reticle's, 1.00 by geometric
mean, in 0.77 times its time and with 9% fewer pips. On the seven small ones it is 1.00 to 1.12
times reticle's, 1.07 by geometric mean, and its time there is mostly building its physical
graph (0.5 to 0.9 s), which a caller routing one graph several times would build once.

Before this, on the three designs then run, `guide:timed` took 6 to 12 times reticle's time
for latest paths 1.17 times reticle's:

| Design | Time, s (before / now) | Latest path, ns (reticle / before / now) |
| --- | --- | --- |
| soc_basys3 | 129.6 / 11.75 | 12.27 / 14.64 / 12.23 |
| apple2_basys3 | 231.5 / 19.29 | 19.74 / 20.31 / 19.24 |
| iso7816_terminal | 108.9 / 4.82 | 8.14 / 10.70 / 8.06 |

What closed it, in the order it was found, on those three designs (each step measured with the
ones before it; the ratios are of latest path and time to reticle's, by geometric mean):

| Step | Latest path | Time |
| --- | ---: | ---: |
| Before | 1.17 | 8.35 |
| A picosecond weighed as reticle weighs it, its exponent of 2 | 1.16 | 0.38 |
| Reticle's guards on critical connections (below) | 1.11 | 0.48 |
| The timing graph timed as reticle times it, criticality against the latest path | 1.13 | 0.60 |
| A long line charged for both its ends | 1.11 | 0.54 |
| Connections with no wire of their own timed | 0.99 | 0.55 |

- **The delay scale.** The harness turned a picosecond into cost by reticle's `delay_scale`
  without its weight, counting the wires the 7-series flow closes at a base cost of a million
  (`TIE_BASE`): that made delay far dearer than reticle makes it under its fitted model, which
  leaves those out and weighs the rest by `FITTED_DELAY_WEIGHT`. Every critical
  search then weighed delay alone, against a lower bound of 35 ps a tile, and flooded: 221
  million expansions on `soc_basys3` where reticle took 33 million, and 7 million after.
- **The model `guide` routes by is now reticle's, to the picosecond.** On the routes of either
  router, `guide`'s analysis of the timing graph and reticle's own (`path_criticality`, with no
  branch load) give the same latest path; the harness prints both. Three things were missing.
  A connection with no wire of its own to route, such as a carry chain's carry from one slice to
  the next, or a flip-flop's reset on the slice's one wire, had no arc, so every path through a
  carry chain was cut and the router never saw the paths reticle found latest. A long line has
  its delay on both of its ends, and reticle's analysis charges both to a route that crosses it
  (`LVB12` and `LVB0` are 322 ps each); a step now charges a node with delays on several names
  all of them, the crossing exactly and a turn back where it came in, which reticle charges
  once, too much. And the cells: each input's own delay to each output (a carry in reaches its
  carry out in a fifth of a select input's time), each pin's own launch and capture, false and
  multicycle paths, and each of reticle's path groups (pads and registers) as a pair of `guide`
  clocks, so that each is weighed against its own latest path as reticle weighs it.
- **Criticality against the latest path**, met or not (`TimingDriven::against_latest`, new), as
  reticle's `against_longest`: `iso7816_terminal` meets its period, and against the period no
  connection of its latest path was near fully critical, still less after the exponent. That
  alone took it from 1.04 to 1.00 times reticle's.
- **Reticle's guards on critical connections**, new in the engine and off by default: a budget
  for a critical connection's search, after which it is searched for again as an uncritical one
  (`TimingDriven::search_budget`), a cap on the present factor a critical search pays
  (`present_cap`, lifted once the overuse stalls), each connection's highest criticality kept
  (`keep_criticality`), and the nets routed most critical first (`critical_first`). With them,
  and the critical nets routed again in the first four iterations as reticle's first passes do,
  `guide:timed` is reticle's settings for the flow. On these designs the budget and the cap
  never came into play; with the timing model right, the other two and the early rerouting
  moved the latest paths by a percent or two either way, design by design, and 1.034 against
  1.042 times reticle's over the 14, for 1.5 times the time (`guide:timed:plain`, without
  them).

What reticle still does that `guide` does not: its refinement after a legal route, which gives
the late lookup tables' inputs their pins again as a matching, and its clean-up passes, which
route the critical signals again once the route is legal (an engine option for them, which
reroutes the critical nets only after a legal iteration and lets the present factor go on
growing, moved nothing measurable, and was left out); and a branch's load, which reticle's
analysis charges and a step delay cannot: up to 0.96 ns of the latest path here. The
small designs, where `guide` is behind, meet their period with three quarters of it to spare,
and their latest paths are a few connections long.

### What it took

- **Aiming at one terminal at a time** (`EngineOptions::target_terminals`, new, off by default).
  Each search of a net's tree used to look for whichever terminal still to connect was
  nearest, with a lower bound that is the least over all their access points. On the 7-series
  a lookup table's sink has six, and that bound was most of a search's work. Aiming each search
  at the terminal nearest the driver, by the lower bound, as reticle and VPR grow their trees,
  took `iso_display` from 17.0 s to 5.0 s and `usb_ulpi_device` from 80.6 s to 32.1 s on the
  same load, with 1% to 2% fewer pips, the same latest paths and both still legal.
- **Physical nodes as the space's nodes**, rather than reticle's graph nodes, which made the
  rule against driving a wire twice hold by construction instead of being found by
  `Routing::verify`.
- **Replaying onto a narrowed graph.** The first replay only closed the nodes `guide` had not
  used, and left every pip between them: reticle's router then took shortcuts through other
  signals' wires, and on `usb_ulpi_device` its negotiation did not settle in 40 passes, though
  `guide`'s routes were legal by the harness's own check. With every unchosen pip leading
  nowhere, each signal has its own tree and nothing else.
- **reticle's estimate, scaled as reticle scales it.** With one figure a tile for every node,
  a clock search on the ECP5, whose network costs a twentieth of a wire, was drawn off it onto
  general routing near its sinks, five flip-flops' clocks among them on `usb_ulpi_device` at a
  bound weight of 1.5. Scaling a tile by the node's own base cost, as reticle's
  `heuristic_base` does, kept them on it.

### What this does not show

- The times are from a shared machine, at loads that moved by a factor of two during the run.
  The ratios are of the two routers measured back to back on each design.
- One placement per design, reticle's with its default seed. Reticle's own flow would have
  placed `spi_console` again only if its route had been congested, which it was not.
- Reticle's router is compared as its flow configures it for each family. On the ECP5 that is
  reticle's defaults, every signal rerouted every pass and no bounding box, where the 7-series
  has reticle's tuned options; `guide` gets the same factors either way, and its incremental
  rerouting on both.
- The UltraScale+, the iCE40 and the Husky are not here, for the reasons under "How".
- What reticle would need to expose so that the replay is not needed: a way to build its
  `Routing` from routes, such as a `Routing::set(Route)`, and for its own `route` to be
  callable from the flow with another router in its place, so that a flow's post-routing steps
  (`apply_lut_moves`, the bitstream, its checks) are not copied out of its command line.

## Placing reticle's designs

reticle, an HDL compiler with its own placer and router for real FPGAs, is expected to adopt
`guide`, so `guide` has to place reticle's own designs on reticle's own devices at least as well
as reticle's placer does. This compares the two, both placements routed by reticle's router and
held to the checks reticle's own flow makes.

### How

`scripts/reticle/place` is a standalone crate that runs reticle's flow (reticle at commit
`1b20721c`, read and built, not changed) up to placement, places with either placer, and runs
the rest of the flow on the result: reticle's router with the options its command line gives
the family, the walk of every sink back to its driver, and, on the ECP5, everything
`reticle fpga --bitstream` checks before it writes a bitstream: block RAMs that do not overlap,
every clock pin reached over a global network, no bit an arc needs clear set by something else,
and **every set bit decoding back through Project Trellis' database to exactly the arcs the
router chose**. reticle's placer runs at the options `reticle fpga` gives it, its default
schedule and effort. Wirelength is reticle's own measure, `place::hpwl`: half-perimeter in tiles
over every signal, clocks included.

`guide`'s placement problem is built from reticle's routing graph and netlist alone, through
reticle's public API:

- a site for every site of the graph, of its kind, at its tile, with the slices of a tile a
  step apart, and columns and rows counted only where the commonest kind has sites, so that a
  carry chain that runs slice to slice and on to the next tile, or up a column past a clock row,
  is a rigid macro;
- cells on package pins fixed there;
- macros from dedicated wiring as reticle finds it: a sink pin that a short walk back through
  the graph reaches from exactly one site of its driver's kind, with each member on its own
  slot of its point (`PlacementProblem::add_macro_in_slots`);
- reticle's site rules as shared resources (`place::Shared`): pins of two sites that are one
  wire carry one signal; sites that exclude each other (a carry and its tables, a distributed
  RAM and six tables) hold one cell; a pool of a tile's own control wires (two clocks and two
  resets an ECP5 tile, a clock enable a slice) carries as many signals as it has wires, a bel's
  output in it a source that carries only its own signal and a way through a lookup table shut
  by a cell on it; an 18-bit block RAM leaves its neighbour empty; and a clock buffer driven by a
  pad is kept off the buffer sites that pad cannot reach, found by a sweep of the graph.

It places by analytic placement with a target density of a half
(`AnalyticOptions::target_density`), then annealing from a temperature of half the spread of
the first moves' cost changes, three times the default moves, with a cost of half-perimeter
wirelength in tiles, clocks left out. The run is one thread, as reticle's is.

Measured on 2026-11-17 with rustc 1.97.1, on an AMD Ryzen Threadripper PRO 9995WX shared with
other work, every run under `nice -n 19` and at most four at a time, reticle at `1b20721c`.
Each design was placed with seeds 1, 2 and 3 by each placer. Placement and routing times are of
seed 1, whose runs of both placers were made at the same load; the counted columns (wirelength,
pips) are means over the seeds whose runs passed, and are the same on any machine.

### Results

**ECP5** (`ecp5-12f-CABGA256`, LFE5U-12F, Project Trellis), every design of reticle's
`testdata/fpga/ecp5` and `testdata/fpga/cynthion` that `reticle fpga --bitstream` builds
for the part: passed is the runs whose route was walked back sink by sink and whose bitstream's
every set bit decoded to exactly the router's arcs. Wirelength is reticle's, in tiles; guide's
ratio to reticle's in brackets.

| design | cells | passed, reticle | passed, guide | place s, reticle | guide | wirelength, reticle | guide | pips, reticle | guide | route s, reticle | guide |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| leds | 6 | 3/3 | 3/3 | 0.2 | 0.3 | 0 | 0 | 0 | 0 | 0.1 | 0.1 |
| button_led | 8 | 3/3 | 3/3 | 0.2 | 0.3 | 46 | 46 (1.000) | 22 | 22 (1.000) | 0.1 | 0.1 |
| block_ram_1bit | 28 | 3/3 | 3/3 | 0.5 | 0.5 | 576 | 572 (0.992) | 255 | 286 (1.122) | 0.3 | 0.3 |
| carry_chain_16 | 60 | 3/3 | 3/3 | 0.6 | 0.5 | 436 | 467 (1.070) | 452 | 451 (0.998) | 0.7 | 0.6 |
| block_ram_2048 | 57 | 3/3 | 3/3 | 0.5 | 0.5 | 1365 | 1386 (1.015) | 636 | 634 (0.997) | 0.8 | 1.0 |
| fifo_sync_16 | 71 | 3/3 | 3/3 | 0.6 | 0.5 | 341 | 363 (1.067) | 747 | 728 (0.975) | 1.1 | 0.9 |
| block_ram_18bit_pair | 96 | 3/3 | 3/3 | 0.5 | 0.5 | 859 | 805 (0.937) | 949 | 938 (0.988) | 1.6 | 1.4 |
| wide_state | 10 | 0/3 | 0/3 | - | - | - | - | - | - | - | - |
| clock_blink | 106 | 3/3 | 3/3 | 0.6 | 0.6 | 159 | 141 (0.889) | 1033 | 1019 (0.986) | 1.2 | 1.6 |
| quiesce | 7 | 3/3 | 3/3 | 0.2 | 0.3 | 0 | 0 | 0 | 0 | 0.1 | 0.1 |
| fifo_sync_64 | 124 | 3/3 | 3/3 | 0.8 | 0.6 | 512 | 621 (1.213) | 1750 | 1790 (1.023) | 1.8 | 1.2 |
| bidir_bus | 155 | 3/3 | 3/3 | 0.7 | 0.6 | 482 | 457 (0.949) | 1560 | 1525 (0.978) | 2.0 | 1.8 |
| target_ulpi_loopback | 118 | 3/3 | 3/3 | 0.6 | 0.6 | 294 | 284 (0.964) | 1182 | 1155 (0.978) | 1.2 | 1.3 |
| lutram_reset_64 | 340 | 3/3 | 3/3 | 1.8 | 1.1 | 1385 | 1407 (1.016) | 6008 | 6001 (0.999) | 5.8 | 7.5 |
| usb_ulpi_device | 1458 | 3/3 | 3/3 | 10.2 | 8.4 | 3482 | 2849 (0.818) | 18207 | 17631 (0.968) | 24.6 | 19.4 |
| usb_bram | 2093 | 3/3 | 3/3 | 21.8 | 17.7 | 6510 | 5742 (0.882) | 27962 | 27081 (0.968) | 39.9 | 26.4 |
| usb_cdc_uart | 2793 | 3/3 | 3/3 | 29.4 | 30.9 | 6802 | 5309 (0.781) | 34649 | 33066 (0.954) | 41.0 | 42.5 |
| nanos_seph_usb | 4103 | 1/3 | 0/3 | - | - | - | - | - | - | - | - |
| smartcard_serial | 3922 | 3/3 | 3/3 | 68.4 | 53.4 | 11403 | 10489 (0.920) | 62049 | 60457 (0.974) | 97.7 | 65.7 |
| usb_hub_target | 3492 | 3/3 | 3/3 | 42.2 | 41.6 | 7984 | 7162 (0.897) | 41498 | 40043 (0.965) | 63.6 | 45.1 |
| usb_inflate | 4405 | 3/3 | 3/3 | 72.3 | 66.5 | 14133 | 13103 (0.927) | 58996 | 56811 (0.963) | 85.8 | 69.3 |
| usb_host_target | 4449 | 3/3 | 3/3 | 66.3 | 65.7 | 11312 | 9950 (0.880) | 55474 | 53826 (0.970) | 76.3 | 69.0 |
| usb_proxy_target | 4551 | 3/3 | 3/3 | 66.0 | 71.5 | 11815 | 9708 (0.822) | 56258 | 53917 (0.958) | 76.0 | 63.3 |
| usb_ulpi_trace | 3326 | 3/3 | 3/3 | 37.5 | 104.5 | 5493 | 3805 (0.693) | 35057 | 31835 (0.908) | 114.2 | 74.3 |
| usb_crypto_console | 11499 | 3/3 | 3/3 | 66.1 | 721.3 | 75079 | 53091 (0.707) | 154657 | 149423 (0.966) | 201.2 | 201.0 |
| usb_mjpeg | 7033 | 3/3 | 3/3 | 138.8 | 164.8 | 22096 | 18520 (0.838) | 89100 | 85539 (0.960) | 119.7 | 89.5 |

Every placement of `guide`'s that reticle's router routed passed every check, as did reticle's.
Over the 22 designs both placed and routed with any wire, `guide`'s placements are **8.6%
shorter** by reticle's own measure, need **1.9% fewer pips**, route in **0.89 times** the time,
and place in 1.14 times reticle's placer's time. On the 11 designs of more than a thousand cells that both pass
`guide` needs fewer pips on every one, 2.6% to 9.2%, with wire 7% to 31% shorter. The small
designs go both ways: `block_ram_1bit`, 28 cells, needs 12% more pips, `fifo_sync_64` 2.3%
more and 21% more wire.

Three rows have no comparison. `wide_state` fails reticle's check that every clock reaches its
flip-flops on a global network, from either placer: a fault of the design or of reticle, not of
either placement. `nanos_seph_usb` fails the bitstream check on two of reticle's three seeds and
all three of `guide`'s, with one set bit that no feature of the database explains; it is
reticle's (below), and with its two block RAM sites kept empty (`--forbid`), `guide`'s three
seeds all pass, at 9,942 tiles of wire and 57,187 pips against reticle's passing seed's 10,872
and 58,871, placed in 54 s against 65 and routed in 70 against 90.

Where `guide` is slow is the largest designs: `usb_crypto_console`, 11,499 cells, takes 721 s
against reticle's 66, and `usb_ulpi_trace` 105 against 38, because annealing at three times the
default moves per temperature, from a warm start, runs 94 million moves on the first; reticle's
placer stops when its anneal stops improving (`flat_steps`). With the default moves per temperature
(`--inner-num 1`), one run each at a lighter load: `usb_crypto_console` places in 208 s to
53,685 tiles and 150,163 pips, and `usb_ulpi_trace` in 30 s to 4,078 tiles and 32,656 pips,
against 53,091 and 149,423, and 3,805 and 31,835, at three times the moves, and reticle's 75,079
and 154,657, and 5,493 and 35,057. Most of the gain is had at a third of the moves; a budget
that scales with the design, or a stop when the anneal stops improving, is left to do.

**7 series** (`xc7a35t-cpg236`, the Basys 3's XC7A35T, Project X-Ray). Passed is the runs whose
lookup tables, given the inputs the router reached them on, walked back sink by sink with no
wire carrying two signals and every clock pin on the global network. Each domain's latest path
is reticle's own routed estimate, by its fitted delay model at the part's grade.

| design | cells | passed, reticle | passed, guide | place s, reticle | guide | wirelength, reticle | guide | pips, reticle | guide | route s, reticle | guide |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| blink | 87 | 3/3 | 3/3 | 0.9 | 1.8 | 251 | 260 (1.039) | 1721 | 2016 (1.171) | 0.6 | 0.7 |
| bram_rom | 18 | 3/3 | 3/3 | 0.8 | 1.9 | 1176 | 1176 (1.000) | 2155 | 2155 (1.000) | 0.8 | 0.8 |
| io_exercise | 195 | 3/3 | 3/3 | 1.5 | 3.0 | 2878 | 2850 (0.990) | 7241 | 7416 (1.024) | 1.1 | 1.1 |
| lutram | 109 | 3/3 | 2/3 | 0.9 | 1.8 | 1215 | 1071 (0.881) | 3479 | 3538 (1.017) | 0.7 | 0.7 |
| pll_blink | 166 | 3/3 | 2/3 | 1.0 | 2.0 | 544 | 582 (1.068) | 3519 | 4127 (1.173) | 0.7 | 0.7 |
| pmod_bidir | 6 | 3/3 | 3/3 | 0.0 | 0.0 | 91 | 91 (1.000) | 212 | 212 (0.998) | 0.0 | 0.0 |
| sw_led | 4 | 3/3 | 3/3 | 0.0 | 0.0 | 38 | 38 (1.000) | 89 | 88 (0.989) | 0.0 | 0.0 |
| soc_basys3 | 2147 | 3/3 | 0/0 | - | - | - | - | - | - | - | - |

| design | latest path, reticle, ns | `guide`, ns |
|---|---:|---:|
| blink (10 ns) | 3.10, 3.33, 3.48 | 4.11, 4.14, 4.24 |
| io_exercise (10 ns) | 3.18, 3.32, 3.36 | 3.86, 4.04, 4.39 |
| lutram (10 ns) | 3.33, 3.49, 3.54 | 3.73, 4.35 |
| pll_blink (`sys`, 10 ns) | 3.24, 3.29, 3.33 | 3.93, 4.06 |

On the designs it places, `guide`'s wire is as short as reticle's (0.995) and needs 5% more
pips, and its latest paths are about 20% longer: it is not timing-driven here, and reticle's
placer, though not timing-driven either by default, draws cells toward their connections with
median moves and packs a flip-flop with the table that feeds it as Vivado does
(`registered_outputs`), which the harness does not model. Two of `guide`'s 21 runs do not route
(`lutram` seed 1, `pll_blink` seed 3): two carry sums from other slices both need one lane's
bypass input. The designs with long carry chains (`blink_carry`,
`carry_probe`, `carry_where`, `partsel`, `spi_console`, the `iso*` and `ssd1306` consoles, the
`soc`, `apple2` and `nes` examples) place legally by `guide`'s model and do not route in reticle's
100 passes, for the same reason; they are not in the table. reticle's own rule for it is a
bipartite matching of each pin's signal to the wires of its lane that pin can reach (`fits`,
`matchable`, private), where `guide`'s shared resources count distinct signals against the
pool's wires, which is right for a pool every pin reaches whole and too lax for a lane whose
pins reach different subsets. `selftest` fails to route from reticle's own placement too.

**iCE40** (`ice40-hx1k-tq144`, reticle's synthetic architecture, through
`fpga::place_and_route`'s steps, the bitstream generated as the check):

| design | cells | passed, reticle | passed, guide | place s, reticle | guide | wirelength, reticle | guide | pips, reticle | guide | route s, reticle | guide |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| blinky_ice40 | 29 | 3/3 | 3/3 | 0.0 | 0.0 | 33 | 37 (1.122) | 150 | 161 (1.073) | 0.0 | 0.0 |
| carry_ice40 | 51 | 3/3 | 3/3 | 0.0 | 0.0 | 90 | 95 (1.056) | 274 | 267 (0.974) | 0.0 | 0.0 |
| ram_ice40 | 37 | 3/3 | 2/3 | 0.0 | 0.0 | 186 | 184 (0.989) | 382 | 352 (0.921) | 0.4 | 0.4 |
| ddr_ice40 | 5 | 3/3 | 3/3 | 0.0 | 0.0 | 0 | 0 | 2 | 2 (1.167) | 0.0 | 0.0 |
| logicram_ice40 | 415 | 3/3 | 3/3 | 0.0 | 0.7 | 1091 | 1016 (0.931) | 4624 | 4737 (1.024) | 0.6 | 0.6 |
| pll_ice40 | 13 | 0/3 | 0/3 | - | - | - | - | - | - | - | - |

The iCE40 designs are small; `ram_ice40` seed 1 does not route from `guide`'s placement (a
block RAM's local input wire wanted by two signals), and `pll_ice40` has no PLL site on the
part for either placer.

### What it took

Three additions to `place`, each additive, so that a problem without them places exactly as
before:

- **Shared resources** (`PlacementProblem::add_shared`, `Item::draw`, `place::Shared`): sites
  that share something of which the items on them may use at most so many distinct values. That
  one form says every rule of reticle's sites `guide` needed: two pins on one wire carry one
  signal, a tile's two clock wires carry two clocks, a carry excludes its tables (the key
  `OCCUPANCY`, under which each item draws itself), a capacity of 0 keeps items off sites, and
  with `add_shared_with_sources`, a bel's output carries its own signal and no other. The
  annealer judges each move by the resources its items' sites draw on, the legalizer seats the
  kinds with fewest sites first and only where the resources allow, and `check` reports
  `Violation::OverShared`. VPR's clusters and nextpnr's `isValidBelForCell` do the same work;
  here it is data in the problem, checked independently.
- **Macro slots** (`PlacementProblem::add_macro_in_slots`): a macro member on a given slot of
  its point, as the four tables beside a 7-series `CARRY4`, each feeding its own input.
- **A target density** (`AnalyticOptions::target_density`). Spreading cut every kind's sites
  where its items parted, over the whole device; on an ECP5 used a few per cent, items the
  solve had drawn to the pads at one edge were spread in a strip along it, as long as the die
  (`usb_ulpi_device`: its lookup tables over 14 by 46 tiles). Each kind is now spread over the
  sites of the smallest window around its items with twice as many sites as items: global
  placement's wirelength went from 27,020 to 18,351 (in guide's units) and, after the same
  annealing, reticle's wirelength from 4,453 to 2,989 tiles.

The rest is the harness's: the problem built from reticle's graph, the flows of three families
copied from reticle's command-line tool, which keeps them in the binary rather than the library,
and a cost in reticle's tiles. Leaving global clock nets out of the problem, which reticle's
router carries on the clock network, took `usb_ulpi_device`'s placement from 17 to 11 s with the
same wirelength. Annealing from a temperature of half the spread of the first moves' cost
changes, rather than the twentieth `AnnealOptions::detailed` starts at, took its wirelength from
2,989 to 2,742 tiles, and from 4,453 to 3,014 before the target density: legalizing a sparse fabric's cells under its site rules leaves them far
from where global placement put them, 6,109 tiles against reticle's final 3,504, and a cold
start cannot undo that.

### What reticle would need to expose

Everything here goes through reticle's public API; reticle was not changed. What was missing:

- **Its flows.** The steps from a database to a routed design for the ECP5 and the 7 series
  (`write_ecp5_bitstream`, `write_xc7_bitstream`) are in the command-line binary, so the harness
  copies some 600 lines of them, which will drift. A library function per family taking a
  placement callback would let `guide` slot in.
- **Its site rules.** `SiteRules`, the legality every move of reticle's placer is judged by, is
  private, as is the audit that checks a placement against it; `guide` rebuilds them from the
  graph. The 7-series lane outputs are where the copy falls short (below). A public
  `SiteRules::find_with` with `refusal(placement, instance, site)` and `audit`, or a
  `legal(placement) -> Vec<String>`, would make every rule exact and checkable;
  `shared_exits` and `route_throughs_needed` are public already and are used.
- **The relations of dedicated wiring** (`build_clusters`, `find_relation`), which the harness
  re-derives with the same walk.

And one fault found on the way, reticle's own: `nanos_seph_usb`'s one block RAM placed at
`X60Y25/EBR0` or `X62Y25/EBR1`, by `guide` or by reticle's placer told to put it there, leaves a
set bit no feature of Project Trellis' database explains (`F73B0` of `MIB_EBR0` at column 60,
`F99B0` of `MIB_EBR2` at column 62), and reticle's flow refuses to write it; its own placer
happens to choose other sites. Those two sites are kept empty for the row of `nanos_seph_usb`
marked so, with the harness's `--forbid`.

### What this does not show

- **Timing-driven placement.** Neither placer was timing-driven here; reticle's
  `--timing-driven` placement and `guide`'s timing cost against reticle's delay model were not
  compared.
- **Gowin and UltraScale+.** reticle's GW2A-18 and XCZU7EV flows need Project Apicula's and
  Project U-Ray's databases, which are not on this machine; the UltraScale+ flow also places
  inside one library function (`uray::zcu104::implement`) with no way in.
- **The 7-series designs with long carry chains**, which do not yet route from `guide`'s
  placements, nor anything bigger than the Basys 3's part.
- **Bitstreams on a board.** The ECP5 checks are those reticle makes before it writes a
  bitstream, every set bit decoding included; no bitstream from these placements was loaded.

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

### Solder mask bridges

`drc::check` also finds solder mask bridges, `Violation::MaskBridge`: copper bare in an
opening of the mask that is another net's. An opening is a pad's, grown by its mask
expansion, with the pad's net, or one drawn on `F.Mask` or `B.Mask`, which takes the net of the
pads in it if they have one, and is of no net otherwise. In an opening of no net, copper of a
net beside copper of another net, or of none, bridges; a pad's hole where the pad has no copper,
as a mounting hole's, is copper of no net there, as KiCad has it.

`curated_mask_bridges_agree_with_kicad` compares the tracks and vias in bridges, which are what
a router adds, on the eleven boards as they come: KiCad finds them on `flea`, 13, in the openings
drawn on its microcontroller module's footprint, and on `nk3-nrf-mini`, a track and a via of
`/LED_R` in the opening over the LED `D1`, as [below](#pcb-routing-of-the-curated-boards); we
find the same 15, and no other. Bridges of pads alone are left out: KiCad also compares pads'
openings with each other, and takes two pads of no net for two nets, as on `flea`'s module,
where it stops at its limit of 199 bridges; those are a board's own, which no router changes.

This came of a bridge that KiCad found in a routed board and we didn't. On `nk3-nrf-mini`, an
opening drawn on `B.Mask` holds a pad of `/3V3` and a plain mounting hole; the reader gave the
opening the pad's net, as the hole has no copper, so a route of `/3V3` went into it, and KiCad
reported the hole and the track as a bridge. Now the reader leaves such an opening of no net,
which the router keeps every net out of, and the check reports any copper with a net in it.

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
- Copper edge clearance and silkscreen checks are KiCad's, and not compared; solder mask
  bridges are compared only for tracks and vias.

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
`RouteOptions::planes` off; [below](#pcb-zones-and-planes), the same boards keep their zones'
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

## PCB routing, searched from both ends

Detailed routing after [a net's searches came to share one tree and to look at a connection
from both ends](architecture.md#decisions), against the router before: at commit `9b71b0f`
on the curated boards, and as released in 0.0.2 through cadlab, which is that router too.
Measured on 2026-11-14, one thread, on the same machine as the rest, which other work shared:
times moved by up to half between runs of the same build, and results did not move at all.
The results after are the same with what `master` had gained by `6cca950`.
Every routing result in the sections above and below was measured before this change, and
none has been measured again: completion, vias and lengths there are those of the router
before, and its times are several times these.

### How

```text
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_APART=0 cargo test --release --features kicad curated_pairs -- --nocapture
```

with the default options, zones and planes included, on the nine curated boards and the two
DRAM boards. And through [cadlab](https://github.com/KarpelesLab/cadlab), which runs
`pcb::route` as a second router: its seven generated boards and its corpus of open-source
boards with their tracks and vias taken out, `CADLAB_BENCH_ROUTER=guide route_bench [--corpus]
<board>`, where it keeps vias out of pads by a keepout over each pad, and counts what its own
check finds open after filling the zones itself. cadlab's own router is beside them, from its
`docs/ROUTER.md`, as are the corpus boards' results before.

### Results

The curated boards. Connections, then vias after cleanup, track in millimetres and seconds,
each before → after:

| Board | Connections | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 8/8 → 8/8 | 0 → 0 | 93 → 93 | 0.06 → 0.06 |
| `TS4231-BOB` | 13/13 → 13/13 | 0 → 0 | 31 → 31 | 0.11 → 0.10 |
| `lisn` | 60/60 → 60/60 | 7 → 7 | 144 → 144 | 0.44 → 0.44 |
| `isi-channel` | 29/29 → 29/29 | 0 → 0 | 453 → 453 | 0.12 → 0.13 |
| `placebo` | 50/50 → 50/50 | 9 → 9 | 292 → 293 | 0.38 → 0.35 |
| `udb-s` | 33/33 → 33/33 | 11 → 6 | 149 → 144 | 1.18 → 0.49 |
| `flea` | 79/79 → 79/79 | 52 → 47 | 1607 → 1611 | 4.66 → 1.61 |
| `GB-LIVE32` | 197/197 → 197/197 | 165 → 202 | 2283 → 2225 | 24.3 → 6.3 |
| `nk3-nrf-mini` | 82/83 → 82/83 | 55 → 61 | 249 → 241 | 18.0 → 5.8 |
| `lpddr4-testbed` | 344/344 → 344/344 | 220 → 235 | 670 → 654 | 85.8 → 9.8 |
| `ddr5-testbed` | 255/255 → 255/255 | 170 → 164 | 876 → 857 | 139.5 → 32.8 |
| All | 1150/1151 → 1150/1151 | 689 → 731 | 6847 → 6746 | 274.6 → 58.0 |

`drc::check` finds nothing on any of them, before or after. Completion is the same, in a fifth
of the time, with 6% more vias, most of them `GB-LIVE32`'s, and 1.5% less track. With
their pairs and lengths (`curated_pairs`, 333 s → 61 s), the five boards routed complete as
before, but `udb-s`'s USB pair no longer finds room side by side where it did, and one of
`ddr5-testbed`'s two matched groups has a net 0.43 mm short of the others where both were met.

cadlab's generated boards, with no via in a pad. Open connections, vias, track and seconds,
before → after, and its own router's seconds over three seeds:

| Board | Connections | Open | Vias | Track, mm | Seconds | cadlab's router, s |
| --- | --- | --- | --- | --- | --- | --- |
| `attiny-2l` | 22 | 0 → 0 | 0 → 0 | 128 → 128 | 0.02 → 0.02 | 0.1 |
| `soic24-2l` | 184 | 0 → 0 | 139 → 159 | 2146 → 2070 | 1.9 → 1.8 | 0.8 to 0.9 |
| `stm32-2l` | 109 | 0 → 0 | 95 → 98 | 1140 → 1142 | 86.1 → 16.4 | 1.1 to 1.3 |
| `stm32-4l` | 109 | 0 → 0 | 48 → 55 | 963 → 949 | 12.3 → 1.9 | 1.7 to 2.5 |
| `qfp-qfn-4l` | 155 | 0 → 0 | 181 → 218 | 1830 → 1864 | 49.9 → 11.4 | 5.5 to 7.5 |
| `fine-4l` | 100 | 0 → 0 | 100 → 109 | 972 → 959 | 139.5 → 7.9 | 1.8 to 2.1 |
| `bga144-4l` | 141 | 14 → 0 | 177 → 232 | 2337 → 2516 | 129.0 → 42.4 | 8.8 to 11.5 |

Every board is now complete, `bga144-4l` with the 14 connections it lacked. Five of the seven
take at most four times what cadlab's router does; `stm32-2l` and `bga144-4l` are the two whose
first pass leaves nets for the pass on the finer lattice, which is most of their time: with a
window of 48 pitches in place of 24, or none, the first pass happens to leave none on
`stm32-2l`, which then takes 3 s. Vias are 13% more over the six boards complete before.

cadlab's corpus, each board one run. Open connections as cadlab counts them, vias, track and
seconds; before is from cadlab's table, where a run was killed at 1200 s and the result is
that of a longer one:

| Board | Layers | Open | Vias | Track, mm | Seconds | cadlab's router: open, s |
| --- | --- | --- | --- | --- | --- | --- |
| `nrfmicro` | 2 | 15 → 9 | 49 → 53 | 461 → 460 | 69.5 → 8.7 | 0, 2 to 24 |
| `buspirate-flash-sop` | 2 | 0 → 0 | 7 → 8 | 523 → 514 | 2.8 → 2.7 | 0, 0.5 to 1.5 |
| `buspirate-rs232` | 2 | 0 → 0 | 6 → 6 | 358 → 358 | 0.5 → 0.5 | 0, 0.5 to 1.2 |
| `lumenpnp-ringlight` | 2 | 0 → 0 | 1 → 1 | 208 → 211 | 0.7 → 1.2 | 0, 1.1 to 1.5 |
| `sweep-v2.2` | 2 | 8 → 8 | 42 → 42 | 2195 → 2195 | 67.4 → 39.1 | 3, 57 to 96 |
| `corne-cherry` | 2 | 95 → 96 | 243 → 275 | 3817 → 3764 | 270 → 20.5 | 2 to 5, 50 to 56 |
| `lumenpnp-mobo` | 4 | 21 → 18 | 474 → 585 | 6485 → 6498 | 559 → 227 | 8 to 14, 45 to 74 |
| `buspirate5-rev10` | 4 | 26 → 14 | 390 → 441 | 3209 → 3518 | 2351 → 228 | 1 to 4, 56 to 66 |
| `cynthion` | 4 | 28 → 16 | 526 → 613 | 3545 → 3687 | 1560 → 202 | 30 to 34, 78 to 100 |
| `tinytapeout-demo` | 4 | no result in 1200 s → 190 | 137 | 1449 | 27.2 | 10 to 25, 59 to 85 |

The boards that took a quarter of an hour and more now take four minutes or less, with fewer
connections open or as many on all but `corne-cherry`. The three largest still take two to
four times what cadlab's router does, and this one is well behind it in completion on `corne-cherry`, `nrfmicro` and
`tinytapeout-demo`, where vias kept out of pads leave pads with no way out that a router with
fanout finds one for; `tinytapeout-demo` is given up on with half its connections open. The two
glasgow boards were not run.

### Where the time goes now

- On `buspirate5-rev10`, 216 of 228 s are detailed routing, 142 million nodes, and four
  fifths of that the pass on the finer lattice: its three supply nets of 62, 70 and 169 pads
  are taken out whole and routed again about ten times each, 88 s between them, whenever a net
  of two pads crosses one of their branches. Taking out only the branch crossed would be the
  next thing to try.
- On `lumenpnp-mobo`, 125 of 227 s are `GlobalSpace::measure` in the second of cadlab's runs,
  which has cadlab's fills of the nets it routes again as fixed zones: each of its probes
  measures to every edge of each fill, as does a step of another net's search near one. And 51 s of the first run are
  zones filled four times. Neither is a search.
- Whether the first pass leaves a net is still what decides a board's time, and it is as
  chaotic as ever: over the variants tried, `stm32-2l` took from 1 to 16 s and `bga144-4l`
  from 18 s up, complete each time.

### What was tried and not kept

All over six of the generated boards, without `attiny-2l`, and the eleven curated ones
together, by connections left open, vias and seconds:

| Variant | Open | Vias | Seconds, three at a time |
| --- | --- | --- | --- |
| As it is | 1 | 1602 | 145 |
| A via's cost in the bound | 4 | 1436 | 144 |
| A look's own path taken as the connection | 6 | 1603 | 163 |
| No window for searches that cross routes | 4 | 1593 | 134 |
| A window of 48 pitches | 4 | 1609 | 117 |
| No glance before the first search | 1 | 1582 | 164 |
| The corridor given up on after the looks | 5 | 1614 | 237 |

The one connection open is `nk3-nrf-mini`'s `/LED_R`, as [above](#results-5). Weighting the
bound, by 1.2, 1.5 and 2, and by 1 to 4 for searches that cross routes, was measured on the
generated boards alone, on an earlier state of this change: searches in the corridor that
find a path expand a quarter of the nodes at 1.5, but with a weight of 2 or 4 on the searches
that cross routes `stm32-2l` and `fine-4l` took 12 to 14 and 17 to 20 s, as nets ripped each
other up far more, and with none on them the six boards took 51 s at 1.5 and 47 s at 1.2
against 40 s at 1, with 11% and 5% more vias.

## PCB routing after the release candidate

What was fixed and added after [the change above](#pcb-routing-searched-from-both-ends),
against it: the regressions it left, a stopped run, vias kept out of pads on parts of fine
pitch, and rip-up of nets of very many pads
([what and why](architecture.md#decisions)). Measured on 2026-11-14, one thread, on the
same machine, which other work shared: times moved by a fifth between runs of the same
build, and results did not move.

### How

```text
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_APART=0 cargo test --release --features kicad curated_pairs -- --nocapture
guide pcb route --strip --no-planes REV10a.kicad_pcb
```

and through cadlab, as above, with its adapter for this API (its branch `guide-adopt`):
`CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_BUDGET_MS=60000 route_bench --corpus <board>` for its
corpus, with a budget of 60 s that most of the boards run out of, and `route_bench <board>`
for its generated boards, both with vias kept out of pads. Before is the commit of the
section above, `6908301`. After, cadlab's adapter also sets `RunControl::fill_on_stop` to
false, as it fills zones itself.

### Results

The curated boards. Connections, vias after cleanup, track in millimetres and seconds, each
before → after:

| Board | Connections | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 8/8 → 8/8 | 0 → 0 | 93 → 93 | 0.06 → 0.06 |
| `TS4231-BOB` | 13/13 → 13/13 | 0 → 0 | 31 → 31 | 0.11 → 0.11 |
| `lisn` | 60/60 → 60/60 | 7 → 7 | 144 → 144 | 0.47 → 0.46 |
| `isi-channel` | 29/29 → 29/29 | 0 → 0 | 453 → 453 | 0.15 → 0.13 |
| `placebo` | 50/50 → 50/50 | 9 → 9 | 293 → 293 | 0.39 → 0.35 |
| `udb-s` | 33/33 → 33/33 | 6 → 12 | 144 → 157 | 0.57 → 0.63 |
| `flea` | 79/79 → 79/79 | 47 → 47 | 1611 → 1611 | 1.77 → 1.71 |
| `GB-LIVE32` | 197/197 → 197/197 | 202 → 189 | 2225 → 2273 | 8.3 → 6.4 |
| `nk3-nrf-mini` | 82/83 → 82/83 | 61 → 55 | 241 → 257 | 6.6 → 6.3 |
| `lpddr4-testbed` | 344/344 → 344/344 | 235 → 232 | 654 → 657 | 13.1 → 11.9 |
| `ddr5-testbed` | 255/255 → 255/255 | 164 → 161 | 857 → 865 | 39.9 → 33.2 |
| All | 1150/1151 → 1150/1151 | 731 → 712 | 6746 → 6834 | 71.4 → 61.2 |

`drc::check` finds nothing on any of them. `udb-s`'s six more vias and 13 mm are its USB pair,
side by side again for 42% of its 32.4 mm (52% of 28.1 mm before the change above); without them the
boards have 25 vias fewer, where that change had added 42: cleanup's search for a route with
fewer vias now has 100,000 nodes for each of a net's first four connections, which saves 13
on `GB-LIVE32` and 6 on `nk3-nrf-mini`, and no more was found to save without the time of
the nets of many pads going up. With their pairs and lengths (`curated_pairs`), the six
boards route as complete as without, `udb-s`'s pair is coupled, and all eleven length
constraints are met, `ddr5-testbed`'s second group included, whose net 0.43 mm short now
gets two meanders of the lowest height between its neighbours.

cadlab's generated boards, with no via in a pad, all complete before and after. Vias, track
and seconds:

| Board | Connections | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- |
| `attiny-2l` | 22 | 0 → 0 | 128 → 128 | 0.02 → 0.02 |
| `soic24-2l` | 184 | 159 → 159 | 2070 → 2070 | 1.3 → 1.2 |
| `stm32-2l` | 109 | 101 → 105 | 1109 → 1243 | 15.1 → 9.3 |
| `stm32-4l` | 109 | 55 → 46 | 949 → 986 | 1.5 → 1.5 |
| `qfp-qfn-4l` | 155 | 185 → 162 | 1933 → 1955 | 8.1 → 7.6 |
| `fine-4l` | 100 | 109 → 109 | 959 → 959 | 6.5 → 5.3 |
| `bga144-4l` | 141 | 233 → 215 | 2517 → 2512 | 36.1 → 35.3 |

46 vias fewer of 842. `stm32-2l` and `bga144-4l` still spend most of their time on the finer
lattice: 6 of 9 s for the 6 nets the first pass leaves on the one, 13 of 35 s for 3 nets on
the other.

cadlab's corpus with a budget of 60 s, vias kept out of pads. Connections as cadlab counts
them after its fanout check, those left open, vias, and the seconds the run took to come
back:

| Board | Layers | Connections | Open | Vias | Seconds |
| --- | --- | --- | --- | --- | --- |
| `buspirate-flash-sop` | 2 | 47 | 0 → 0 | 7 → 7 | 2.3 → 2.3 |
| `buspirate-rs232` | 2 | 43 | 0 → 0 | 6 → 6 | 1.6 → 1.6 |
| `lumenpnp-ringlight` | 2 | 37 | 0 → 0 | 1 → 1 | 0.9 → 0.9 |
| `nrfmicro` | 2 | 85 | 13 → 13 | 33 → 33 | 8.9 → 8.3 |
| `sweep-v2.2` | 2 | 48 | 6 → 6 | 44 → 44 | 57.1 → 49.7 |
| `corne-cherry` | 2 | 432 → 434 | 91 → 63 | 289 → 438 | 13.2 → 48.1 |
| `tinytapeout-demo` | 4 | 368 | 121 → 56 | 291 → 473 | 54.1 → 49.9 |
| `buspirate5-rev10` | 4 | 533 → 473 | 160 → 24 | 316 → 482 | 53.0 → 49.4 |
| `lumenpnp-mobo` | 4 | 529 → 517 | 71 → 56 | 518 → 438 | 65.6 → 51.6 |
| `cynthion` | 4 | 728 | 82 → 63 | 513 → 528 | 60.0 → 51.8 |
| `glasgow-revC3` | 4 | 748 → 746 | 346 → 83 | 267 → 405 | 49.5 → 49.1 |
| `glasgow-revD1` | 6 | 1748 → 1608 | 791 → 659 | 635 → 736 | 66.8 → 50.8 |
| All | | | 1681 → 1023 | | |

No run has an error of cadlab's check before or after, but for `corne-cherry` before, where
the escapes of `right/D+` and `right/D-` lay on one line: two regions' ways out of a cluster
were each found before either was placed. Where the open connections went:

- *Neck-down* is `tinytapeout-demo`'s 65, measured with it off and on, and most of
  `glasgow-revC3`'s 263: their nets' class widths do not clear the pads beside their own on
  the 0.4 and 0.5 mm parts, and they are now routed at the widest width that does. Both
  boards then run out of the 60 s.
- *The routes kept for a stopped run*, with the nets that rip-up had out when the budget
  ended as they were before it took them out, are most of the rest, on the boards that run
  out of their budget: `buspirate5-rev10`, `cynthion`, `lumenpnp-mobo` and `glasgow-revD1`.
  The two were not measured apart on these.
- `corne-cherry` has 28 fewer open, and takes most of its budget where it gave up after
  13 s: the nets that now leave their pads are routed, and rip each other up.
- A run now comes back when its budget ends: `lumenpnp-mobo` and `glasgow-revD1` were 6 and
  7 s late, filling zones.

Rip-up by branches, for a net of 32 terminals or more with no plane, on `buspirate5-rev10`
routed by `guide pcb route --strip --no-planes`, which routes its three supply nets of 62,
70 and 169 pads as tracks: 193 s and 114 million nodes before, 135 s and 72 million after,
complete both times, with 433 and 406 vias. With its planes, the default, its supply nets
are stubs to them, which are still taken out whole, and the board routes as before.

### What was tried and not kept

- *Branches of smaller nets, and of nets with planes.* At 8 terminals `nrfmicro` went from
  13 open connections to 28: its ground of 24 pads was cut sixteen times, then taken out
  whole and left with no way back to its first pad. Cutting the stubs of nets with planes
  lost `ddr5-testbed` one to three of its 255 connections in each of seven variants: in the
  first pass or only after, with the dead ends kept or pruned, counted towards the limit of
  rip-ups or not, at 4, 8 or 16 cuts, and with the last rip-up whole, which alone kept all
  255 without pairs first and lost three with.
- *More nodes for cleanup's search on every net.* 100,000 nodes for each of up to 16
  connections saved 52 vias more on `lpddr4-testbed`, with 116 mm more track, in 32 s where
  it takes 12.
- *A smaller budget for searches that cross nothing, in the passes on the finer lattice.* An
  eighth: `stm32-2l` 9 → 13 s, `bga144-4l` 35 → 43 s. A thirty-second: 9 → 6 s and 35 → 32 s.
  Complete each time, and no trend to choose by.
- *The last fill of the joining of zones' pieces as the result's.* It is the same fill only
  when every zone keeps its islands, which none of cadlab's does.

### What this does not show

- No fanout: a pad that leaves its footprint but is walled in by its neighbours' routes
  still has no via beside it planned before routing. Of the 1023 connections open, most are
  on `glasgow-revD1`, which runs out of its budget with two fifths unrouted, and on parts of
  fine pitch on the other boards.
- Where the time of a board with large zones goes: on `lumenpnp-mobo` with a budget of
  300 s, 142 to 160 s for both of cadlab's runs, of which about 105 s are between the end of
  cleanup and the result, filling zones and joining their pieces, and planning on them
  before routing, 4 to 17 s a fill.
- Fixed zones in parts changed nothing that was measured: `lumenpnp-mobo` took 161 s without
  and 160 s with, with the same result, and `GlobalSpace` is built in under 0.1 s in both of
  its runs either way. The 125 s that `GlobalSpace::measure` took on that board with
  cadlab's adapter for 0.0.2, which gave its fills as fixed zones, were not reproduced with
  its adapter for this API, so what the parts save on such input is not shown here.
- `lumenpnp-mobo` has two nets that the router takes for routed and cadlab's check finds in
  two pieces, before and after: not looked into.
- cadlab counts three vias of `corne-cherry` and of `lumenpnp-mobo` in the boxes of pads with
  no hole, before and after, with vias kept out of pads: its count is by a pad's box, and the
  router's rule by its copper.
- The results of the other sections were not measured again.

## PCB routing with a budget

What was changed for a host that gives a run a budget, cadlab with its 60 s, against
[the release above](#pcb-routing-after-the-release-candidate), `7fa55e4`
([what and why](architecture.md#decisions)). Measured on 2026-11-15, one thread, three runs at
a time on a machine that other work shared: a run that its budget cuts ends where the clock
finds it, so what such a run leaves open moves with the machine's load, by a few connections
on the boards run more than once.

### How

Through cadlab's adapter, with its corpus as above, on an uncommitted patch of it that sets
`RouteOptions::fills` to false and gives the smallest via its rules allow as
`RouteOptions::fanout_via`:

```text
CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_BUDGET_MS=60000 route_bench --corpus <board>
CADLAB_BENCH_ROUTER=guide route_bench <board>
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_APART=0 cargo test --release --features kicad curated_pairs -- --nocapture
```

### Results

cadlab's corpus with a budget of 60 s, of which the run has 48, with vias kept out of pads.
Connections left open as cadlab counts them on the board that it fills again, vias, track
and the seconds the call took, each before → after; `cut` is a run that its budget stopped.
cadlab's own router, with its seed 1 and three threads under the same 60 s, is beside it:

| Board | Layers | Connections | Open | Vias | Track, mm | Seconds | cadlab's router: open, seconds |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `buspirate-flash-sop` | 2 | 47 | 0 → 0 | 7 → 7 | 497 → 497 | 2.4 → 1.8 | 0, 0.4 s |
| `buspirate-rs232` | 2 | 43 | 0 → 0 | 6 → 6 | 358 → 358 | 1.7 → 1.1 | 0, 0.5 s |
| `lumenpnp-ringlight` | 2 | 37 | 0 → 0 | 1 → 1 | 209 → 209 | 0.9 → 0.9 | 0, 0.4 s |
| `nrfmicro` | 2 | 85 | 13 → 11 | 33 → 37 | 428 → 423 | 8.7 → 9.1 | 0, 12.7 s |
| `sweep-v2.2` | 2 | 48 | 6 → 6 | 44 → 42 | 2250 → 2251 | 49.7 → 49.6, cut | 3, 53.7 s |
| `corne-cherry` | 2 | 434 → 443 | 63 → 97 | 438 → 441 | 5526 → 5250 | 46.9 → 49.2, cut | 1, 30.1 s |
| `tinytapeout-demo` | 4 | 368 → 372 | 56 → 47 | 473 → 478 | 4620 → 4817 | 49.9 → 49.9, cut | 6, 60.8 s |
| `buspirate5-rev10` | 4 | 473 → 474 | 24 → 17 | 482 → 510 | 3229 → 3399 | 49.4 → 49.4, cut | 2, 28.8 s |
| `lumenpnp-mobo` | 4 | 516 → 512 | 58 → 50 | 434 → 476 | 6197 → 6270 | 51.0 → 51.3, cut | 10, 62.6 s, cut |
| `cynthion` | 4 | 728 → 730 | 63 → 32 | 528 → 580 | 3330 → 3531 | 51.7 → 52.3, cut | 28, 61.4 s, cut |
| `glasgow-revC3` | 4 | 746 → 745 | 83 → 69 | 405 → 366 | 3077 → 3113 | 49.2, cut → 47.6 | 35, 60.5 s, cut |
| `glasgow-revD1` | 6 | 1608 | 659 → 399 | 736 → 783 | 5175 → 5109 | 50.7 → 50.8, cut | no result |
| All | | | 1025 → 728 | | | | |

No run has an error of cadlab's check, before or after. On the eleven boards that cadlab's
router has a result for, 366 → 329 against its 85. `glasgow-revC3` now ends on its own; so
did `corne-cherry` before, 1 s from its deadline, and no longer does. Left to end, with a
budget of 300 s: `lumenpnp-mobo` 17 → 8 open in 154 → 81 s, `corne-cherry` 63 → 62 in
58 → 153 s, `glasgow-revC3` 69 in 46 s where it had 71 in 206 s.

cadlab's generated boards, whose budget of 120 s none but the last comes near. All complete
before and after, but `big-4l`, which is cut with 377 connections open before and 56 after.
Vias, track and seconds:

| Board | Connections | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- |
| `attiny-2l` | 22 | 0 → 0 | 128 → 128 | 0.02 → 0.02 |
| `soic24-2l` | 184 | 159 → 159 | 2070 → 2070 | 1.5 → 1.5 |
| `stm32-2l` | 109 | 105 → 102 | 1243 → 1349 | 11.0 → 11.7 |
| `stm32-4l` | 109 | 46 → 57 | 986 → 944 | 1.6 → 1.7 |
| `qfp-qfn-4l` | 155 | 162 → 159 | 1955 → 1935 | 9.0 → 6.6 |
| `fine-4l` | 100 | 109 → 103 | 959 → 943 | 6.3 → 7.3 |
| `bga144-4l` | 141 | 215 → 200 | 2512 → 2459 | 42.0 → 44.0 |
| `big-4l` | 1275, of which made 898 → 1219 | 1715 → 1958 | 16678 → 18805 | 97.1 → 98.0, both cut |

The curated boards, which are routed with no budget, are as they were to the via and the
millimetre, all eleven, with their pairs and lengths: 1150 of 1151 connections, 712 vias,
6834 mm.

Where the open connections went:

- *A run with a budget routes for the most connections soonest.* On `glasgow-revD1` the
  first pass took its ground of 584 pads out whole, twice, for a stub in another net's way,
  and with it two more supplies of a hundred pads: when the budget ended they were out, and
  the run went back to routes from before, with most nets never tried. Now such a net loses
  the stubs in the way only, and a net that finds no way in its corridor waits until every
  net has had its turn before it is searched for over the board and takes others out: 659
  open → 399, with 26 → 28 million nodes. The same on `cynthion`, 63 → 32. A first version
  that made a net wait after its search over the board, and searched it again when its turn
  came, left `glasgow-revD1` with 598: 12 of 31 million nodes went into the searches made
  twice.
- *A host's fill measured to by its parts.* Before anything was routed, `glasgow-revD1`
  spent 9.1 s finding out that nothing was closer to cadlab's fills than their clearance,
  each item measured against every edge of a fill, twice: 2.4 s now, `cynthion` 2.2 → 0.7 s,
  `lumenpnp-mobo` 3.9 → 0.9 s, with the same fills.
- *No last fill for a host that fills again*, `RouteOptions::fills`: 11 s of
  `lumenpnp-mobo`'s 48.
- *Pours.* `lumenpnp-mobo`'s motor drivers have a pour of 2 by 4 mm from each sense pin to
  its resistor. They were planes: the two pads were joined, the net had nothing to route,
  the routes of the pins beside went through the pour, and the pins were left apart and
  shut in, eight of them, two with no cause to give, `Cause::Unknown`, which cadlab reads
  as a net that the router took for routed. They are copper now. With a budget of 300 s the
  board ends with 8 open, where it had 17.
- *`corne-cherry` is behind.* It ended on its own a second before its deadline, with 63
  open, and now it is cut in its second pass with 97: the nets that wait come back when
  the others are placed, and take more of them out, 541 turns in the first pass. Left to
  end it has 62, as before, in 153 s where it took 58.

### What was tried and not kept

- *A pour's net routed as tracks* in place of its pour as copper. `corne-cherry` took 158 s
  where it took 58, for 62 open against 63, and the curated `lisn` 216 mm of track where it
  has 144, the tracks inside its own pours.
- *Any zone of a net of four pads or fewer as a pour.* `GB-BRK-LINK-A`'s ground has four
  pads and a zone under the whole board, which the other nets then went around, 101 mm of
  track where they have 93, and `ddr5-testbed`'s supplies have areas of 7 by 9 mm and more
  under its parts. A pour is now a zone of such a net that is 32 pitches of the lattice
  wide or less: 2 by 4 mm on `lumenpnp-mobo`, half a millimetre on `corne-cherry`.
- *Waiting, and stubs cut, in a run with no budget.* On the curated boards the wait cost
  `nk3-nrf-mini` a connection, 16 vias and 54 mm, and `GB-LIVE32` 6 vias; the stubs cost
  `ddr5-testbed` a connection, as [before](#what-was-tried-and-not-kept-1). A run with no
  budget is as it was.
- *Dog-bones for every ball inside an area array*, with vias kept out of pads: a via
  halfway to a ball diagonally beside it, the same way for the whole array, with the
  fan-out via where the net's own does not fit, and pads up to a pitch further apart than
  a track's passage taken as a cluster, as `glasgow-revC3`'s balls of 0.35 mm at 0.8 mm
  are, 0.1 µm more than a track and its clearances apart. 44 dog-bones there and 130 on
  `cynthion`: `glasgow-revC3` 69 open → 118 with a budget of 60 s, and 78 with 300 s, cut
  both times where it ended in 46 s; `cynthion` 23 → 69. The balls that have a via can be
  searched for, at length, on the one layer that the board's two planes leave them, where
  the lattice has no line midway between two vias: with a lattice of a sixth of the
  array's pitch, 0.1333 mm, 92 open at 60 s and 71 at 300 s.
- *`RouteOptions::fanout_via` alone*, which is kept, changed nothing here: on these boards
  no pad is trapped that the smaller via gets out, as a track leaves the balls of
  `glasgow-revC3` on paper.

### What this does not show

- Why `glasgow-revC3`'s 59 balls and `tinytapeout-demo`'s 32 pins stay open: a way between
  two pads or two vias that is as wide as a track and its clearances, or a few micrometres
  more, is on the lattice by chance. Nothing here puts a line of the lattice in the middle
  of such a way.
- `glasgow-revD1` is cut with 398 connections not yet tried or made: 7 s of its 48 are
  global routing, 4 s the board before it, and the rest 28 million nodes.
- `lumenpnp-mobo` is cut while its zones' pieces are joined, 17 s after 8 s of cleanup, so
  cadlab's second run does not start, and 50 pads stay off their pours.
- The passes on the finer lattice take most of a budget where they are reached, 35 of 46
  million nodes on `corne-cherry`, for few connections.
- The fan-out via and `RouteOptions::fills` are measured through cadlab alone.

## PCB routing in parts

What was changed after [the release with a budget](#pcb-routing-with-a-budget), `3b0aba3`,
for the same host, cadlab, whose adapter now gives a run half of its 60 s and routes again
on the board that it has filled itself with the rest
([what and why](architecture.md#decisions)). Measured on 2026-11-15, each run with one
thread, three runs at a time, on a machine that other work shared: a run that its budget
cuts ends where the clock finds it, and what it leaves open moves by a few connections
from one run to the next.

### How

Through cadlab's adapter as it is at its commit `d564baf`, with nothing changed but the
crate:

```text
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_BUDGET_MS=60000 route_bench --corpus <board>
RAYON_NUM_THREADS=3 CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_BUDGET_MS=60000 route_bench --corpus <board>
CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_WORK=20000000 route_bench --corpus <board>
guide pcb route --strip <board>
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_APART=0 cargo test --release --features kicad curated_pairs -- --nocapture
```

The second line is cadlab's own router, one run at a time with three threads. The third
bounds a run by its work, which gives the same result on any machine, and is what the ways
tried were compared by.

### Results

cadlab's corpus with a budget of 60 s, vias kept out of pads. Connections left open as
cadlab counts them on the board that it fills, vias, track and the seconds the call took,
each before → after, and cadlab's own router beside them:

| Board | Layers | Open | Vias | Track, mm | Seconds | cadlab's router: open, seconds |
| --- | --- | --- | --- | --- | --- | --- |
| `buspirate-flash-sop` | 2 | 0 → 0 | 7 → 7 | 497 → 497 | 1.8 → 1.8 | 0, 0.5 s |
| `buspirate-rs232` | 2 | 0 → 0 | 6 → 6 | 358 → 358 | 1.0 → 1.0 | 0, 0.4 s |
| `lumenpnp-ringlight` | 2 | 0 → 0 | 1 → 1 | 209 → 209 | 0.8 → 0.9 | 0, 0.4 s |
| `nrfmicro` | 2 | 11 → 8 | 37 → 44 | 423 → 430 | 8.2 → 12.1 | 0, 13.3 s |
| `sweep-v2.2` | 2 | 6 → 6 | 42 → 42 | 2250 → 2231 | 57.1 → 52.3, cut | 3, 57.0 s |
| `corne-cherry` | 2 | 58 → 51 | 507 → 492 | 5880 → 6041 | 58.9 → 51.5, cut | 3, 30.4 s |
| `tinytapeout-demo` | 4 | 47 → 44 | 478 → 485 | 4817 → 4809 | 34.8 → 42.6, cut | 7, 60.4 s |
| `buspirate5-rev10` | 4 | 17 → 13 | 510 → 513 | 3399 → 3426 | 35.5 → 45.0, cut | 2, 28.2 s |
| `lumenpnp-mobo` | 4 | 12 → 9 | 800 → 770 | 6617 → 6606 | 58.0 → 55.8, cut | 15, 62.3 s, cut |
| `cynthion` | 4 | 38 → 23 | 561 → 594 | 3392 → 3645 | 60.8 → 56.2, cut | 27, 61.8 s, cut |
| `glasgow-revC3` | 4 | 69 → 69 | 412 → 419 | 3140 → 3169 | 40.1 → 41.0, cut | 36, 60.4 s, cut |
| `glasgow-revD1` | 6 | 349 → 344 | 875 → 872 | 6084 → 6067 | 58.7 → 56.0, cut | no result |
| All | | 607 → 567 | | | | |

No run has an error of cadlab's check, before or after. On the eleven boards that cadlab's
router has a result for, 258 → 223 against its 93. No call takes longer than its budget
after, where `cynthion`'s took 60.8 s before. With the adapter told not to join zones'
pieces, `RouteOptions::join` off, as it fills and routes again itself, the twelve boards
had 570 open in a run of the same build: no gain there, so the adapter may leave it on.

The seven largest with a budget of 300 s, open connections and the seconds the call took:

| Board | Open | Seconds |
| --- | --- | --- |
| `tinytapeout-demo` | 36 → 36 | 76 → 63 |
| `buspirate5-rev10` | 16 → 12 | 161 → 139 |
| `corne-cherry` | 97 → 61 | 164 → 115 |
| `lumenpnp-mobo` | 8 → 5 | 82 → 73 |
| `cynthion` | 11 → 11 | 179 → 116 |
| `glasgow-revC3` | 69 → 66 | 46 → 260 |
| `glasgow-revD1` | 117 → 115 | 298 → 282 |
| All | 354 → 306 | |

The curated boards, routed with no budget by `guide pcb route --strip`, connections, vias,
track and seconds, before → after:

| Board | Connections | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 8/8 → 8/8 | 0 → 0 | 93 → 93 | 0.10 → 0.07 |
| `TS4231-BOB` | 13/13 → 13/13 | 0 → 0 | 31 → 31 | 0.17 → 0.16 |
| `lisn` | 60/60 → 60/60 | 7 → 7 | 144 → 144 | 1.08 → 0.86 |
| `isi-channel` | 29/29 → 29/29 | 0 → 0 | 453 → 453 | 0.16 → 0.15 |
| `placebo` | 50/50 → 50/50 | 9 → 9 | 293 → 293 | 0.54 → 0.52 |
| `udb-s` | 33/33 → 33/33 | 12 → 12 | 157 → 157 | 0.93 → 0.82 |
| `flea` | 79/79 → 79/79 | 47 → 47 | 1611 → 1611 | 1.85 → 1.57 |
| `GB-LIVE32` | 197/197 → 197/197 | 189 → 189 | 2273 → 2273 | 7.8 → 6.6 |
| `nk3-nrf-mini` | 82/83 → 82/83 | 55 → 55 | 257 → 257 | 6.6 → 6.5 |
| `lpddr4-testbed` | 344/344 → 344/344 | 232 → 232 | 657 → 657 | 14.9 → 12.1 |
| `ddr5-testbed` | 255/255 → 255/255 | 161 → 161 | 865 → 865 | 38.6 → 30.7 |
| All | 1150/1151 → 1150/1151 | 712 → 712 | 6833 → 6833 | 72.8 → 59.9 |

All eleven are as they were to the via and the millimetre, with the same nodes searched,
and so are their pairs and lengths (`curated_pairs`): a run with no budget has no gates.
With gates in its passes on the finer lattice, `ddr5-testbed` had five vias fewer and
23 mm less routed as above, and with its pairs routed first, as `curated_pairs` routes
it, a connection fewer, `RESET_N`.

Where it went:

- *A budget in parts.* `corne-cherry`, 2 layers and a ground pour on each: its first pass
  ends after 11 s with 373 of its connections made, and the two passes on the finer
  lattice then take others' routes out 380 times each, for a minute each, and end where
  they began. A run that its budget stopped there went back to its best routes and
  returned, with the 40 or so stubs that join its pours' pieces not made: 97 open with a
  budget of 300 s, where the run left to end had 62. Routing now ends for the joining to
  have its time, 61. The same for the pours of `lumenpnp-mobo` and `buspirate5-rev10`.
- *The phases before routing.* On `lumenpnp-mobo`'s second run, on the routed board, the
  fill that nets are planned on took 10.7 s, measured against every edge of cadlab's fills
  for each via in them, and the places for the stubs of a ground of 250 pads 5 s, asked of
  the same fills: by their parts, 5.5 s and 1.1 s. The fill made to tell the planes is the
  one that nets are planned on where nothing is added to the board in between, which this
  board's pours are. `cynthion`'s 38 → 23 is its second run, which now has the time to
  route.
- *Search nodes by block.* The same routes for a sixth less time on the curated boards,
  72.8 → 59.9 s, and `glasgow-revC3`'s first run, which ends on its own with the same 27
  million nodes, in 39.9 s for 45.8.
- *Gates*, in the passes on the finer lattice of a run with a budget: `nrfmicro` 11 → 8.

Gates with a budget of 20 million nodes for each board, so that each way has the same
work, open connections with none, in the passes on the finer lattice, and in every pass:

| Board | None | Finer passes | Every pass |
| --- | --- | --- | --- |
| `nrfmicro` | 11 | 8 | 13 |
| `buspirate5-rev10` | 30 | 30 | 33 |
| `cynthion` | 32 | 32 | 42 |
| `tinytapeout-demo` | 65 | 65 | 76 |
| `glasgow-revC3` | 71 | 72 | 135 |
| `corne-cherry` | 94 | 94 | 89 |

### What was tried and not kept

- *Gates in every pass.* The 59 balls of `glasgow-revC3`'s array that were shut in are then
  not: they are routed through the channels of one track between the balls of the two
  outer rows, each in place of another, and the first pass takes routes out for them until
  its time is over: 123 open with a budget of 60 s where it had 69, and 78 after 241 s of a
  budget of 300. Through the vias of a fan-out nothing passes on this board at all: its
  rules keep a track 0.25 mm from a hole, and two vias of 0.2 mm drill at 0.8 mm leave it
  0.225 mm. A fan-out for the whole array was not tried again for that reason.
- *A weight on the bound*, 1.5, for the searches of a run with a budget. Nodes were queued
  again once reached more cheaply, and the same boards took more of them: `corne-cherry`
  85 million in its 48 s for 46. With each node expanded once at most, fewer nodes, 40
  million, of which more were new and cost what a new node costs, so no more was routed in
  the time, and `glasgow-revD1` had 1029 vias for 783.
- *Rip-up given up for a pass after a million nodes that made no more connections*, the
  best routes put back and the nets left routed with nothing taken out: `glasgow-revD1`
  344 open → 195, the one board that is cut in its first pass with most nets waiting, and
  every other board the other way, `buspirate5-rev10` 16 → 61, `corne-cherry` 54 → 68,
  `glasgow-revC3` 69 → 114, `tinytapeout-demo` 40 → 73.
- *No rip-up in the first pass until every net has had its turn*: `glasgow-revD1` 344 →
  387, as its ground's stubs are among what waits, `corne-cherry` 54 → 60. For the nets
  that wait alone: 361 and 78.

### What this does not show

- Why rip-up does not settle: `corne-cherry` ends with some 40 pads of its two
  microcontrollers shut in by the routes of the pads beside them, after 760 routes taken
  out, where cadlab's router leaves 3 connections open in 30 s. It is most of what is left
  on the two-layer boards.
- `glasgow-revD1` is cut with its first pass not over, 344 open, and has 115 with a budget
  of 300 s. Global routing is 6.6 s of its first run's 30, and a node of a search still
  costs over a microsecond: after the numbering by block, the index of shapes a fifth of
  it, the heap and the bounds another.
- `glasgow-revC3`'s array: 66 open at best. A way out for each ball has to be planned for
  all of them at once.
- Cleanup still looks for a route with fewer vias for a ground that is stubs to its plane,
  5 s of `lumenpnp-mobo`'s second run for nothing to find, and the fills before a run on a
  routed board are still 11 s of its 18 there.
- cadlab counts vias in the boxes of pads with no hole, 3 to 7 on four boards, before and
  after: each is beside a round test point, in the corner of its box and clear of its
  copper, as `RouteOptions::vias_in_pads` asks.
- cadlab's own router gave no result for `glasgow-revD1`: it stopped on an index out of
  bounds.
- The generated boards of cadlab were not measured again.

## PCB routing with ways out planned

What was changed after [the release in parts](#pcb-routing-in-parts), `cc1ad2f`, 0.0.5, for
the same host, cadlab, whose adapter now turns the joining of zones' pieces off in both of
its runs ([what and why](architecture.md#decisions)). Measured on 2026-11-15, each run with
one thread, three runs at a time, on a machine that other work shared: a run that its budget
cuts ends where the clock finds it, and what it leaves open moves by a few connections from
one run to the next, and by up to ten on a board from one order of the nets to another.

### How

Through cadlab's adapter as it is on its branch `guide-005`, commit `c17907f`, with nothing
changed but the crate:

```text
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_BUDGET_MS=60000 route_bench --corpus <board>
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_SEED=<1|2> CADLAB_BENCH_BUDGET_MS=60000 route_bench --corpus <board>
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide route_bench <board>
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
GUIDE_ROUTE=1 GUIDE_APART=0 cargo test --release --features kicad curated_pairs -- --nocapture
```

cadlab's own router is as cadlab measured it for that commit, on the same machine, with its
seed 1 and three threads. The ways tried were compared over three orders of the nets, no
seed and seeds 1 and 2, on the nine boards that do not complete, as one run of a board
says little: with the release, 162 connections open on `corne-cherry` over the three, 45 on
`buspirate5-rev10`, 66 on `cynthion`.

### Results

cadlab's corpus with a budget of 60 s, vias kept out of pads. Connections left open as
cadlab counts them on the board that it fills, vias, track and the seconds the call took,
each before → after, with a second run of after in brackets, and cadlab's own router beside
them:

| Board | Layers | Open | Vias | Track, mm | Seconds | cadlab's router: open, seconds |
| --- | --- | --- | --- | --- | --- | --- |
| `buspirate-flash-sop` | 2 | 0 → 0 | 7 → 7 | 497 → 497 | 1.0 → 0.8 | 0, 4, 460, 0.5 |
| `buspirate-rs232` | 2 | 0 → 0 | 6 → 6 | 356 → 356 | 0.9 → 0.7 | 0, 10, 281, 0.5 |
| `lumenpnp-ringlight` | 2 | 0 → 0 | 1 → 1 | 209 → 209 | 0.4 → 0.2 | 0, 2, 180, 0.4 |
| `nrfmicro` | 2 | 7, 5, 5 → 7, 5, 5 | 55 → 50 | 453 → 454 | 24.7 → 24.2 | 0, 46, 454, 12.5 |
| `sweep-v2.2` | 2 | 6, 7, 6 → 6, 7, 6 | 42 → 42 | 2250 → 2231 | 54.9 → 55.0, cut | 3, 21, 2161, 56.8 |
| `corne-cherry` | 2 | 46, 37, 40 → 45, 37, 35 | 581 → 609 | 7433 → 7606 | 57.1 → 56.8, cut | 3, 483, 8276, 33.7 |
| `tinytapeout-demo` | 4 | 39, 36, 42 → 40, 36, 44 | 474 → 454 | 5114 → 4932 | 34.1 → 32.9, cut | 8, 273, 4910, 60.3 |
| `buspirate5-rev10` | 4 | 6, 7, 5 → 11, 9, 5 | 437 → 476 | 3526 → 3523 | 44.8 → 35.0, cut | 2, 313, 3053, 31.9 |
| `lumenpnp-mobo` | 4 | 13, 6, 7 → 11, 5, 7 | 727 → 743 | 6414 → 6337 | 44.5 → 38.5, cut | 15, 344, 5432, 62.8, cut |
| `cynthion` | 4 | 26, 23, 30 → 20, 22, 27 | 646 → 608 | 3999 → 3744 | 57.7 → 43.4, cut | 27, 478, 3177, 61.8, cut |
| `glasgow-revC3` | 4 | 36, 40, 40 → 34, 37, 40 | 589 → 577 | 4142 → 4062 | 56.8 → 56.7, cut | 35, 413, 3422, 60.2, cut |
| `glasgow-revD1` | 6 | 174, 216, 191 → 152, 144, 128 | 1071 → 1192 | 7860 → 8489 | 58.9 → 59.3, cut | 201, 1008, 8785, 63.0, cut |
| All, first order | | 353 → 326 | | | | 294 |
| The nine, three orders | | 1096 → 925 | | | | |

No run has an error of cadlab's check, before or after. On the eleven boards without
`glasgow-revD1`, 179 → 174 against cadlab's 93: what was gained is on the boards that a
budget cuts while they still make connections, `glasgow-revD1`, which is now ahead of
cadlab's own router in each of the three orders, and `cynthion`. The boards whose pads
stay shut in are where they were: `corne-cherry`, `tinytapeout-demo`, `glasgow-revC3`,
`nrfmicro`, `sweep-v2.2`. `buspirate5-rev10` has 25 over the three orders for 18: it had
17 and 18 in two runs of the three before the change to cleanup below, and 19, 25 and 25
in three with it, which is what its second run finds on tracks that were pulled tight. A run of the
nine boards while the machine was busier, load 13, had 996 over the three orders, with
`glasgow-revD1` at 171, 136 and 183 and `nrfmicro`'s first run cut a second from its end.

The seven largest with a budget of 300 s, one order: open connections, vias and the
seconds the call took, before → after:

| Board | Open | Vias | Seconds |
| --- | --- | --- | --- |
| `tinytapeout-demo` | 45 → 45 | 398 → 398 | 36 → 34 |
| `buspirate5-rev10` | 0 → 0 | 537 → 537 | 94 → 87 |
| `corne-cherry` | 45 → 28 | 813 → 701 | 159 → 133, cut |
| `lumenpnp-mobo` | 10 → 10 | 722 → 722 | 46 → 41 |
| `cynthion` | 11 → 11 | 580 → 580 | 61 → 52 |
| `glasgow-revC3` | 37 → 33 | 498 → 504 | 192 → 149, cut |
| `glasgow-revD1` | 79 → 80 | 1485 → 1463 | 289 → 288, cut |
| All | 227 → 207 | | |

The four that end on their own have the routes they had, to the via and the millimetre,
in a tenth less time.
`corne-cherry`'s 45 and 28 are one run each of a board that moves by that much from one
run to the next.

The curated boards, routed with no budget by `guide pcb route --strip`: connections, vias
and track, each before → after, the nodes searched, which are the same, and seconds:

| Board | Connections | Vias | Track, mm | Nodes | Seconds |
| --- | --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 8/8 → 8/8 | 0 → 0 | 93 → 93 | 4603 | 0.08 → 0.07 |
| `TS4231-BOB` | 13/13 → 13/13 | 0 → 0 | 31 → 31 | 6556 | 0.16 → 0.14 |
| `lisn` | 60/60 → 60/60 | 7 → 7 | 144 → 144 | 9309 | 0.85 → 0.71 |
| `isi-channel` | 29/29 → 29/29 | 0 → 0 | 453 → 453 | 39191 | 0.16 → 0.14 |
| `placebo` | 50/50 → 50/50 | 9 → 9 | 293 → 293 | 125068 | 0.54 → 0.47 |
| `udb-s` | 33/33 → 33/33 | 12 → 12 | 157 → 157 | 648354 | 0.94 → 0.68 |
| `flea` | 79/79 → 79/79 | 47 → 47 | 1611 → 1611 | 1799004 | 1.81 → 1.38 |
| `GB-LIVE32` | 197/197 → 197/197 | 189 → 189 | 2273 → 2273 | 7334256 | 6.87 → 5.40 |
| `nk3-nrf-mini` | 82/83 → 82/83 | 55 → 55 | 257 → 257 | 2475553 | 6.59 → 5.81 |
| `lpddr4-testbed` | 344/344 → 344/344 | 232 → 232 | 657 → 657 | 5758807 | 12.2 → 9.67 |
| `ddr5-testbed` | 255/255 → 255/255 | 161 → 161 | 865 → 865 | 19302561 | 28.8 → 24.5 |
| All | 1150/1151 → 1150/1151 | 712 → 712 | 6833 → 6833 | 37503262 | 58.9 → 49.0 |

With their pairs and lengths (`curated_pairs`) the six boards are as they were too, row
for row: the same connections, vias and track, the same pairs side by side for the same
share of their length, 42% on `udb-s`, 46% on `GB-LIVE32`, 52%, 62% and 53% on
`ddr5-testbed`, and all eleven length constraints met to the same lengths, in 57.8 s for
60.5 s.

cadlab's generated boards, whose run has a budget of 120 s, all complete before and
after. Vias, track and seconds, before → after:

| Board | Connections | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- |
| `attiny-2l` | 22 | 0 → 0 | 128 → 128 | 0.02 → 0.02 |
| `soic24-2l` | 184 | 159 → 159 | 2070 → 2076 | 1.2 → 1.2 |
| `stm32-2l` | 109 | 85 → 76 | 1149 → 1174 | 1.5 → 1.4 |
| `stm32-4l` | 109 | 43 → 44 | 992 → 996 | 1.3 → 1.3 |
| `qfp-qfn-4l` | 155 | 173 → 173 | 1928 → 1942 | 5.0 → 4.8 |
| `fine-4l` | 100 | 99 → 100 | 955 → 950 | 3.3 → 3.3 |
| `bga144-4l` | 141 | 233 → 255 | 2559 → 2598 | 12.7 → 14.9 |
| `big-4l` | 1275 | 2121 → 2011 | 21908 → 21606 | 66.6 → 63.8 |
| All | | 2913 → 2818 | 31690 → 31470 | |

They differ, as a run with a budget does: its global routing ends sooner, with other
corridors for some nets, and its nets are pulled tight before they are searched for fewer
vias. `bga144-4l` has 22 vias more for it and `big-4l` 110 fewer; cadlab counts 48 sharp
corners on `big-4l` where it counted 865.

Where it went, in the seconds of a run of cadlab's on the boards measured, before → after:

- *The parts of a fill.* The fills that a host gives are measured to by their parts, and
  each fill was cut into them two or three times a run. `lumenpnp-mobo`: 1.8 s of its
  first run and 5.8 s of its second, 7.6 s of 60, → 0.9 s and 2.2 s; the plan of its
  second run, before anything is searched, 6.4 → 3.0 s. `cynthion` 6.4 → 3.0 s over its
  two runs, `corne-cherry` 1.3 s.
- *A host's fill cut in windows.* `glasgow-revD1`: the fill of a plane of 118 000 edges,
  for one via of an escape in it, 1.66 s → under 0.1 s, and its three planes for the
  fifteen vias of its second run 5.1 → 0.23 s. The plan of that run 7.7 → 3.1 s, and of
  its first 3.2 → 1.5 s.
- *Global routing that ends when it gains nothing.* `glasgow-revD1`: the overuse of its
  tiles falls from 356 to 178 in four iterations, 0.4 s, and is 176 to 183 for the
  twenty-six after: 6.5 → 1.1 s in its first run, 2.2 → 0.5 s in its second. With the
  two above, its first run begins to search after 2.6 s where it began after 9.7 s, and
  its second 3.6 s after it was called where it took 9.9 s.
- *One query for the steps from a node*, and the fixed items packed: the same nodes in a
  sixth less time on the curated boards, 58.9 → 49.0 s. With all of the above
  `glasgow-revD1` searches 47 to 53 million nodes in its 60 s where it searched 34 to 35.

- *Tight before fewer vias.* The tracks of a board that a budget cuts, one run each, with
  the connections they had: `lumenpnp-mobo` 6722 → 6342 mm, `cynthion` 3914 → 3748 mm,
  `glasgow-revD1` 8734 → 8590 mm, `tinytapeout-demo` 5003 → 4932 mm, and `corne-cherry`
  the other way, 7400 → 7606 mm.

What cadlab's corpus showed that is not this crate's to change: `tinytapeout-demo`'s 46
open against 8. Its QFN of 80 pins at 0.4 mm has pads 0.2 mm apart, its board a clearance
of 0.2 mm, and its supplies and analog nets classes of 0.24 and 0.25 mm: no track of such
a net leaves its pad past the pads beside it, nor does a track of the pad beside a supply
pin, at any width, and 29 of the 46 were such pads, three by three. The board's own rules
allow 0.16 mm inside the part's courtyard and 0.18 mm from its pads
(`tinytapeout-demo.kicad_dru`), and cadlab's adapter gave neither to the router, as each
names a kind of item or a footprint, which `RuleSpec::areas` has no place for. With the
courtyard's rule given as an area, at the larger of the two clearances, on an uncommitted
change of the adapter: 40, 36 and 44 open → 18, 20 and 20 over the three orders, with
nothing that cadlab's check takes out, against 8 for cadlab's own router; and with the
adapter as it was for 0.0.5 and the release, 46, 42 and 44 → 24, 22 and 19.

And `lumenpnp-mobo`, 7 open with 0.0.5 and 13 with 0.0.6, which was not looked into then:
over three orders of the nets it is 7, 8 and 6 with 0.0.5 and 13, 6 and 7 with 0.0.6, so
one order of three. Of the six commits between the two, the first with the 13 is the one
that plans the ways out (`9e7afac`; 7 at `6ae6e9d` before it, 12 at it). With it the
first pass leaves three nets that the passes on the finer lattice then search for until
routing's share of the time is over, 18 s for one connection, where the run used to end
on its own after 19 s: the run is then one that was stopped, cleanup has two seconds, and
the host's second run starts later.

### What was tried and not kept

All for the pads that stay shut in, over three orders of the nets at 60 s, connections
open, the three summed. `corne-cherry` has 127 as released, through the adapter for 0.0.5:

- *Lanes.* A pad of a row of fine pitch has one way out at either end, as wide as the
  row's pitch, and the routes of the pads beside it turn across it, or end in a via at
  its end: with a router that plans its fan-out, cadlab's own, each track of a row leaves
  it straight, and the vias are further out or under the part. A lane at either end of
  each such pad, for as far as a via and its clearances take, in which a track of another
  net cost three times its length and a via of another net four more: `corne-cherry`'s
  tracks then left its rows straight, and it had 117 open. With the lanes that lead away
  from a part closed to other nets' vias, and a history of conflicts by cell of two
  pitches: 98, 103, 111, 120, 121 and 128 in six variants of the costs. And on the boards
  of four and six layers, whose rows have their vias at the ends of their pads:
  `buspirate5-rev10` 19 → 27 with the lanes alone, and 43 with the history,
  `glasgow-revD1` 775 → 1067 and 1127, `lumenpnp-mobo` 26 → 37, `nrfmicro` 17 → 21.
- *Throats*: other nets' tracks forbidden in the first two pitches of a lane, within
  three quarters of the row's pitch of the pad's axis, where one is in the way of the
  pad's own track whatever else is routed. `corne-cherry` 146 to 167: with every track
  out of a row straight, the pads in the middle of a row have a way out and no place for
  a via beyond it between their neighbours' tracks, which run on beside them.
- *The history of conflicts by cell of two pitches*, alone: `corne-cherry` 137,
  `buspirate5-rev10` 25 for 19, `glasgow-revD1` 823 for 775.
- *The lattice's pitch from the necked widths*, 0.2 mm for 0.22, half the pitch of
  `corne-cherry`'s rows: 111 with the lanes and the history, as without.
- *The nets of rows of fine pitch first*: 128 with the same.
- *The passes on the finer lattice ended* once they had gone on for half the nodes of
  the run before them, a million at least, with no connection more: the nine boards 886
  for 878, `lumenpnp-mobo` 23 for 27 and done in 28 s for 39, `nrfmicro` 27 for 17 and
  `glasgow-revC3` 126 for 113.

- *Sixteen rip-ups of a net before it is locked*, for four, in a run with a budget, as a
  net that is locked is a wall to those after it: `corne-cherry` 129 for 117 with the
  same build, `glasgow-revC3` 129 for 111, and `tinytapeout-demo` the same routes.

With a budget of 300 s `corne-cherry` had 27 open with the lanes and the history, as it
has without, and its passes were over after 146 s: time is not what its rows lack.

### What this does not show

- Why rip-up does not settle, still. Of the connections that the twelve boards leave
  open, two in three are to pads that are shut in. What the lanes showed is that the
  tracks of a row can be made to leave it straight by a price and that this is not
  enough: on two layers a row of fourteen pads at 0.4 mm needs its vias staggered at two
  or three depths outside it and some under the part, each pad's with the others' in
  view, and a search for one net at a time places each where it is nearest.
- What a route through a host's fill cuts off. With no fill at the end and no joining, a
  run counts its connections on the fill that it planned on, and a host that fills again
  finds pads off their pours: a second run of the host's joins most of them, and 7 of
  the 353 connections that the twelve boards had open with 0.0.6 were such pads. The
  run's own account of them would take a fill of the routed board, which is what the
  host's adapter turns off for its time.
- Vias and track. A run of 60 s has two seconds for cleanup on the boards that a budget
  cuts, and has two times the vias of cadlab's router on `lumenpnp-mobo` and 1.7 times
  on `tinytapeout-demo`. Not looked into here.
- The plan of a second run still cuts the host's new fills into parts once, 2 to 3 s on
  the three largest boards, by halves with a clip of the whole for each.
- A node of a search still costs most of a microsecond: the curated boards take 1.3 µs
  for each node of a whole run where they took 1.6. The walk down the index is still a
  sixth of a search, the heap and the numbering of nodes a quarter.
## PCB routing with a second wait

What was changed after [the release above](#pcb-routing-with-ways-out-planned), 0.0.7, for
the same host, cadlab, whose adapter gave guide's first run 0.45 of its 60 s because one
board of six layers made nothing more after 20 s and the eleven others wanted the run
longer ([what and why](architecture.md#decisions)). Measured on 2026-11-16 and 17, each
run with one thread, three runs at a time, on a machine that other work shared with a
load of 8 to 14: a run that its budget cuts ends where the clock finds it, and the same
build left 1096 connections open over three orders on one hour and 851 on another, of
which `glasgow-revD1` 629 and 424. What is compared here was run within the same hour.

### How

Through cadlab's adapter as it is on its `master`, commit `ecf0242`, with nothing changed
but the crate, and then with the share of its first run read from the environment, on an
uncommitted change of it:

```text
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_TRACE=1 CADLAB_BENCH_BUDGET_MS=60000 [CADLAB_BENCH_SEED=2|3] route_bench --corpus <board>
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_TRACE=1 CADLAB_BENCH_WORK=40000000 route_bench --corpus <board>
GUIDE_ROUTE=1 cargo test --release --features kicad curated_boards_route -- --nocapture
```

cadlab's own router is as cadlab measured it for 0.0.7, on the same machine, with its seed
1 and three threads. With a budget of work the adapter makes one run of guide and no
second: what cadlab then counts open is what one run leaves on the board that cadlab fills
again, and the run's own count, on the fill that it planned on, is in the trace.

### Results

cadlab's corpus with a budget of 60 s, vias kept out of pads, in three orders of the
nets, no seed and seeds 2 and 3. Connections left open as cadlab counts them on the board
that it fills again: before → after through the adapter as it is, whose first run has
0.45 of the budget, and after with a first run of 0.7; vias, track and the seconds the
call took are of the first order, before → after with 0.7:

| Board | Layers | Open, as the adapter is | Open, the first run with 0.7 | Vias | Track, mm | Seconds | cadlab's router: open, vias, track, seconds |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `buspirate-flash-sop` | 2 | 0 → 0 | 0 | 7 → 7 | 497 → 497 | 0.8 → 0.7 | 0, 4, 460, 0.4 |
| `buspirate-rs232` | 2 | 0 → 0 | 0 | 6 → 6 | 356 → 356 | 0.7 → 0.7 | 0, 10, 281, 0.4 |
| `lumenpnp-ringlight` | 2 | 0 → 0 | 0 | 1 → 1 | 209 → 210 | 0.3 → 0.3 | 0, 2, 180, 0.4 |
| `nrfmicro` | 2 | 7, 5, 5 → 4, 7, 7 | 4, 7, 7 | 50 → 60 | 454 → 495 | 23.8 → 23.6 | 0, 46, 454, 11.9 |
| `sweep-v2.2` | 2 | 6, 7, 6 → 6, 8, 6 | 6, 7, 6 | 42 → 42 | 2231 → 2231 | 55.0, cut → 56.1, cut | 3, 21, 2161, 52.6 |
| `corne-cherry` | 2 | 45, 40, 35 → 41, 36, 43 | 35, 35, 38 | 609 → 820 | 7606 → 8422 | 56.8, cut → 46.7, cut | 3, 483, 8276, 30.1 |
| `tinytapeout-demo` | 4 | 13, 20, 20 → 9, 13, 6 | 9, 8, 5 | 572 → 605 | 5530 → 5799 | 39.8, cut → 48.7, cut | 6, 274, 4913, 61.1 |
| `buspirate5-rev10` | 4 | 7, 5, 5 → 7, 7, 7 | 2, 6, 3 | 513 → 501 | 3650 → 3816 | 31.5, cut → 37.4 | 2, 313, 3053, 28.4 |
| `lumenpnp-mobo` | 4 | 10, 5, 7 → 10, 10, 6 | 9, 10, 6 | 789 → 748 | 6557 → 6610 | 38.9, cut → 40.5 | 15, 347, 5424, 62.5, cut |
| `cynthion` | 4 | 20, 22, 25 → 19, 23, 28 | 19, 14, 11 | 601 → 629 | 3748 → 3908 | 42.9, cut → 57.5, cut | 28, 476, 3177, 61.4, cut |
| `glasgow-revC3` | 4 | 34, 38, 40 → 40, 39, 41 | 38, 38, 37 | 575 → 560 | 4060 → 3983 | 56.7, cut → 57.8, cut | 36, 415, 3436, 60.7, cut |
| `glasgow-revD1` | 6 | 155, 141, 128 → 165, 156, 119 | 162, 136, 123 | 1186 → 1085 | 8414 → 7954 | 59.0, cut → 58.0, cut | 192, 1046, 8967, 60.7, cut |
| All | | 297, 283, 271 → 301, 299, 263 | 284, 261, 236 | | | | 285 |
| Without `glasgow-revD1` | | 142, 142, 143 → 136, 143, 144 | 122, 125, 113 | | | | 93 |
| All, three orders | | 851 → 863 | 781 | | | | |

No run has an error of cadlab's check, and none has anything that its verification took
out.

Through the adapter as it is, nothing is gained: 851 → 863. What changes is where the
connections are made. `glasgow-revD1`, 1948 connections on the board as the run counts
them: before, the first run has 317 open when it is cut at 27 s and cadlab's second run,
on the routed board, makes 161 of them; after, the first run has 181 open at 27 s and the
second makes 16. With 0.7 the first run has 170 open at 42 s and the second makes 8. The
board ends near 160 either way, and no longer needs its first run short: with 0.0.7 and
a first run of 0.7 cadlab measured it at 821 over three orders for 454 with 0.45, and
the twelve boards at 1189 for 890; here it has 421, and the twelve 781. The eleven others
are what a longer first run is for: 427 → 360, of which `cynthion` 67 → 44,
`tinytapeout-demo` 53 → 22 and `corne-cherry` 120 → 108. `tinytapeout-demo`'s are the
supply pins of its QFN, necked by its courtyard's clearance, and the pours: 53 → 28
through the adapter as it is.

Against cadlab's own router at the same 60 s, three orders against its one run: 284, 261
and 236 open on the twelve boards against 285, and 122, 125 and 113 against 93 on the
eleven without `glasgow-revD1`. Ahead in every order on `glasgow-revD1`, `cynthion` and
`lumenpnp-mobo`; within what two runs differ by on `tinytapeout-demo`, `buspirate5-rev10`
and `glasgow-revC3`; behind on `corne-cherry`, 35 to 38 against 3, `nrfmicro`, 4 to 7
against none, and `sweep-v2.2`, 6 or 7 against 3.

`lumenpnp-mobo`'s 9, 10 and 6 are all pads of its ground, and none of them is this
crate's: its run counts itself complete, 524 of 524. The host gives a run the nets that
its board has open, and the ground of this board has none while nothing is routed, as the
host's fill of its pour on the top layer joins every pad. So the first run is not asked
to route the ground, its routes go through that pour, and the second run, on the routed
board, is asked for some 220 stubs to the ground's plane at once, of which five to ten
find their pads shut in. With the nets that have zones among those of the first run, on
an uncommitted change of the adapter, the run makes those stubs while the board is free,
813 of 813 connections in 20 s for 524 of 524 in 29 s, and the board has 0, 1 and 0
open, with 713 vias for 748: the twelve boards 275, 252 and 232, the eleven 113, 116 and
108.

What one run does, by the six orders of the nets of `corne-cherry` with a budget of 40
million nodes and no second run, before the wait and with the rows planned: connections
open as the run counts them on the fill it planned on, and after the stroke as cadlab
counts them on the board that it fills again:

| `corne-cherry`, by order of the nets | Run's count / cadlab's | Sum |
| --- | --- | --- |
| 0.0.7 | 42/101, 26/88, 33/88, 33/100, 19/92, 41/100 | 194 / 569 |
| Rows planned | 23/85, 25/86, 25/85, 26/87, 24/82, 31/78 | 154 / 503 |
| And the pours joined, 97% of the work for routing | 28/43, 26/52, 25/35, 29/47, 24/42, 31/42 | 163 / 261 |
| And the stubs' ends kept | 42/53, 32/43, 22/44, 31/45, 29/44, 35/49 | 191 / 278 |

With the pours joined at its end, what cadlab finds open after one run is half of what it
was: the grid tells some sixty pads off their pour in a run, in a dozen pieces, and a stub
to each piece joins most of them. Through the host's two runs that is less, as its second
run joined most of them before: at 60 s with the first run at 0.7 and the wait, over three
orders, the eleven boards have 358 open with the pours joined for 381 without,
`corne-cherry` 106 for 118 and `tinytapeout-demo` 22 for 28. Of what `corne-cherry` still
has open after its two runs, a dozen are pads off their pours all the same: its first
run counts 22 open at 42 s and cadlab 34 on the board filled again, where its second run
then moves nothing.

The rows planned, the same way: 367 for 358 on the eleven boards, `corne-cherry` 99 for
106, `nrfmicro` 15 for 18, `glasgow-revC3` 127 for 113 and `buspirate5-rev10` 14 for 11.
With them `nrfmicro`'s call is back after 5 s for 24 and `lumenpnp-mobo`'s after 24 s for
42, with no more made of the time. They are planned only if asked.

The first run's share, with the wait and the pours, over three orders: 0.45 863, 0.6
786, 0.7 781 and 780 in two runs, 0.8 821; the eleven boards 423, 374, 360 and 358, 367.

The curated boards, routed with no budget, are as they were, 1150 of 1151 connections
with 712 vias and 6833 mm of track in 46 s: none of this is in a run with neither a
budget nor a deadline.

### What was tried and not kept

- *The rows planned, by default.* Above: a gain on `corne-cherry` and a loss on two
  boards of four layers, 367 for 358. `RouteOptions::row_fanout` is off unless asked.
- *Runs that move what the runs before placed*, through the host, on an uncommitted
  change of cadlab's adapter: after its two runs, `eco::reroute_with` with the routed
  copper within 1.5 mm of the ends of what is still open taken out, in another order of
  the nets each time, a run on the board filled again after each, kept when cadlab counts
  fewer open, for as long as there is time. Over three orders `nrfmicro` 12 for 18 and
  `buspirate5-rev10` 9 for 11, and nothing on `lumenpnp-mobo` and `tinytapeout-demo`,
  where a run that took out the copper round five to eleven open pads left 50 to 70 open
  when it was cut: a net with no zone that touches an area is taken out whole, and a
  board of four layers is then routed again by half. At 3 mm and 4.5 mm no run of any
  board made a connection more. Eight connections of 360 for four more runs of each
  board: not kept in the adapter.
- *A pad beside others taken off the fill that joins it*, on boards whose host does ask
  for the nets with zones. A pad with no hole, with a pad of another net closer than a
  track and its clearances, was made a terminal of its own though its net's fill joins
  it, to be given a stub like the pads that the fill does not reach, to the fill of
  another layer where there is one: `corne-cherry` 101 for 108 over three orders and
  `cynthion` 41 for 44, and `glasgow-revC3` 677 for 113 and `buspirate5-rev10` 74 for 11,
  as most pads of a dense board have such a neighbour, the balls of a BGA among them, and
  a ground of two hundred terminals more spends its searches on stubs that it did not
  need. Then only the pads with such neighbours on two opposite sides and not on four,
  as in a row, each given its stub through a via before anything is routed, as fixed
  copper: 343 for 337 on the eleven boards, `corne-cherry` 113 for 109.
- *An end to a first run that has stopped gaining*, which is what the host asked for.
  Not built: what stopped `glasgow-revD1` gaining was the order of its searches, and with
  the wait its first run holds at 27 s all but 25 of the connections that its two runs
  made by 54 s. The turns that
  cross other nets' routes then make nothing more on that board, 1767 connections at
  27 s and 1778 at 42 s, and neither does a second run: a rule that told that would save
  the time and have nothing to spend it on.

### What this does not show

- A way out of the pads of a QFN of 0.4 mm on a board of two layers. `corne-cherry` has
  35 to 38 open against 3 for a router that plans its fan-out and keeps it: two in three
  are pads of its two QFNs shut in by the tracks and vias of the pads beside them. The
  rows planned as routes are cut like any other, and planned as walls they were worse.
- What a route through a host's fill cuts off for good. The stubs made once routing is
  over join what still has a way, and a pad shut in by then has none: a dozen on
  `corne-cherry`, whose ground is a pour on both of its two layers. A net that its fill
  joins whole has no route to make and nothing that keeps count of its pour.
- Rip-up that settles. `glasgow-revD1` makes eleven connections in fifteen seconds of
  turns that cross other nets' routes, and `nrfmicro` ends on its own after 23 s with 4 to
  7 open and the rest of its minute unspent.
- A via on a via was this, found after the release and gone with 0.0.9: once in some forty
  runs of `corne-cherry` here, as twice in cadlab's record before, a second run put a via
  of a net with a zone where the board it was given has one of the same net, and cadlab's
  verification took the piece out. A track that ends at a via is two of its net's places
  at one point, the end on its layer and the via on all of them, and the first of the two
  was asked which layers the net joins there. From the layer that the track is on, that
  was the track's end, which joins none; from any other, the via. A search goes through
  such a via from the other layers only, where nothing has to be added; a path found from
  its far end is laid the other way round, and was then given a via of its own on the one
  it went through. The layers joined at a point are now those of all the net's places
  there that share one, whichever is asked from. On seven boards of the corpus over three
  orders, 21 runs, the old question would have had another answer 12 times, in 8 runs:
  mostly where the net's own run had the via, which one net's copper holds once.
- Vias and track. `corne-cherry` has 820 vias for 609, and the corpus about as many
  more as it has connections more: 1.7 times cadlab's router's on `corne-cherry`, twice
  on `lumenpnp-mobo` and `tinytapeout-demo`.
- The load of the machine. One build, two hours: 1096 and 851.

## PCB routing settled

What was changed after [the release above](#pcb-routing-with-a-second-wait), 0.0.9, for
the same host, cadlab, and the first of what that section does not show: rip-up that
settles ([what and why](architecture.md#decisions)). Measured on 2026-11-17, each run with
one thread, six runs at a time, on a machine that other work shared with a load of 70 to
75 on its 64 threads: a run with a deadline made half the nodes that it made for the
section above, and `glasgow-revD1` left 250 connections open in a minute where it left
162. So what is compared by is a budget of work, which is the same run on any machine
and under any load.

### How

Through cadlab's adapter as it is on its `master`, commit `5a50d3a`, with nothing changed
but the crate:

```text
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_WORK=40000000 [CADLAB_BENCH_SEED=2|3] route_bench --corpus <board>
```

After a run that its budget of work stopped the adapter makes no second, and cadlab
counts what the one run leaves open on the board that it fills again.

### Results

cadlab's corpus with a budget of 40 million nodes, vias kept out of pads, in three orders
of the nets, no seed and seeds 2 and 3. Connections left open as cadlab counts them,
before → after; vias, track and the seconds the call took are of the first order:

| Board | Layers | Open | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- | --- |
| `nrfmicro` | 2 | 4, 7, 9 → 4, 8, 8 | 62 → 62 | 499 → 497 | 54 → 20 |
| `sweep-v2.2` | 2 | 6, 7, 6 → 6, 7, 6 | 42 → 42 | 2231 → 2231 | 108 → 111 |
| `corne-cherry` | 2 | 58, 57, 54 → 50, 43, 47 | 799 → 524 | 8253 → 5855 | 60 → 79 |
| `tinytapeout-demo` | 4 | 14, 19, 5 → 14, 19, 5 | 608 → 608 | 5685 → 5685 | 89 → 93 |
| `buspirate5-rev10` | 4 | 2, 6, 5 → 0, 6, 5 | 501 → 502 | 3816 → 3818 | 78 → 74 |
| `lumenpnp-mobo` | 4 | 0, 1, 0 → 0, 0, 1 | 713 → 709 | 6544 → 6596 | 67 → 69 |
| `cynthion` | 4 | 25, 17, 25 → 21, 17, 25 | 659 → 613 | 3899 → 3618 | 92 → 94 |
| `glasgow-revC3` | 4 | 36, 38, 37 → 36, 35, 36 | 563 → 563 | 3937 → 3937 | 156 → 147 |
| `glasgow-revD1` | 6 | 170, 145, 139 → 170, 145, 139 | 1067 → 1067 | 7616 → 7616 | 77 → 80 |
| All, three orders | | 892 → 853 | | | |

`buspirate-flash-sop`, `buspirate-rs232` and `lumenpnp-ringlight` are complete before and
after, in under a second. No run has an error of cadlab's check.

`corne-cherry` is where it tells: 169 → 140. Its first pass has 395 of 432 connections
after 4 million nodes; before, the passes went on, taking nets out for each other, and
had 396 at best 23 million nodes later. Now the first pass ends after 7 million nodes
and the passes on the finer lattice after 12 million, with 397 to 400, and then come the
chains: 32 to 45 tried in each order, 6 to 8 kept, which make 12 to 15 of the connections
and pieces of fill that were open, in 6 to 7 million nodes, after which the run ends on
its own with half its budget. Its vias and track in the first order are those of another
run altogether, as they are from one order to the next: 433 vias and 5345 mm in the
second order before.

`nrfmicro` ends after 20 s for 54 with the same 20 open over the three orders: its passes
stop where they stopped gaining, and of the 6, 30 and 18 chains tried one is kept. Each
of its open connections is to a pad of its USB receptacle or its module with the routes
of four other nets round it, and a chain that moves those has a fifth to move.
`glasgow-revD1`, `cynthion`, `glasgow-revC3` and two orders of `tinytapeout-demo` and
`buspirate5-rev10` spend their 40 million nodes in their passes, which end sooner or not
at all, and have no chain: `glasgow-revD1` is the same run. `sweep-v2.2` has one chain
kept in two orders, for a piece of its pour, and what it has open has no route across
other nets' either: its searches end at their limits of nodes.

With the ways out of rows of fine pitch planned (`RouteOptions::row_fanout`, which a host
turns on), and the chains: `nrfmicro` 3, 5 and 3 open for 4, 8 and 8, `corne-cherry` 42,
46 and 42 for 50, 43 and 47, and on four layers `buspirate5-rev10` 12, 3 and 0 for 0, 6
and 5, `tinytapeout-demo` 18, 16 and 9 for 14, 19 and 5, `glasgow-revC3` 109 for 107 and
`cynthion` the same. The boards of two layers gain and those of four lose, as
[before](#pcb-routing-with-a-second-wait), by less: the option stays off unless asked.

Runs with a deadline are in the host's record: on this machine in this hour a minute
made half the nodes it had made, and two runs of one build differed by 77 connections in
one order of `glasgow-revD1`.

The curated boards, routed with no budget, are as they were, 1150 of 1151 connections
with 712 vias and 6833 mm of track: a run with neither a budget nor a deadline settles
nothing and ends no pass early.

### What was tried and not kept

All at 40 million nodes, connections open as cadlab counts them.

- *Chains that are kept with a net left cut*, if the routes then make more connections
  in all, and that go on while they still could: with a deadline of 60 s `corne-cherry`
  had 122 open over three orders for 120 without them, its chains thirteen nets long
  with seven more connections open at their end than at their start, and undone. With
  every cut net whole again or the chain ended there, 276 over six orders for 321.
- *Chains half as long and half as deep, at three times the price of a crossing*: six
  nets, two deep, 30 mm for a crossing, 60,000 and 100,000 nodes for a cut net's
  searches. `corne-cherry` 144 over three orders for 135, `nrfmicro` the same, with
  twice the chains tried.
- *Windows.* A connection that no chain made, tried once more with the nets of few pads
  that have a pad within eight pitches of its open pads taken out whole, eight at most,
  and routed again after it, the one that found no way first the next time, three
  times. `corne-cherry` 145 over three orders for 140, every other board the same to the
  connection: a net taken out whole comes back by another way, and what it is then in
  the way of is somewhere else.
- *Seven tenths of routing's share for the passes and the rest for the chains*, with no
  end to a pass that has stopped gaining: `corne-cherry` 277 over six orders for 274 with
  the passes ended, and `nrfmicro` 29 for 31, in three times the seconds.
- *The count without the pours.* With chains counted by the connections of the routes
  alone, `corne-cherry`'s run counted 23 open of its own and cadlab 40 to 51 on the board
  filled again: a chain made a connection through the ground's pour and left two pads
  of the ground behind its track. Counted with the pieces of fill that are cut off, and
  with a chain for each net that has such pieces, the run's count and its host's are 34
  and 45 in that order, and the six orders 276 for 274: the same, with a count that says
  what the host will find.

- *Chains for the pads cut off their fills* were in 0.0.10 and are gone with 0.0.11. A
  net with pieces of fill cut off had chains of its own before the others, each piece a
  terminal to join to its fill again across what is in the way. They made no more: 235
  open over three orders on `nrfmicro`, `corne-cherry`, `buspirate5-rev10`,
  `lumenpnp-mobo`, `cynthion` and `sweep-v2.2` with them and 232 without, through the
  host's adapter with the rows planned on two layers. And they cost time that a budget of
  work does not count: `lumenpnp-mobo`'s first run, with a deadline at 42 s on a machine
  under load, was settling from 25.2 s to 39.0 s with 193 of its 193 nets routed, a
  ground of two hundred pads searched for at four times a search's limit, its cleanup
  had 3 s for 7, and it was back too late for its host's second run, which is what joins
  the pads off its pours: 4, 8 and 5 open for 2, 1 and 5. Without those chains the call
  takes 40.8, 36.7 and 33.4 s for 41.1, 45.8 and 37.8 s, as with 0.0.9. The pieces are
  counted, and joined by the stubs after routing.
- *Longer chains for what is left.* Once chains of twelve nets, three deep, across four,
  in four tries, have made what they can: twice those, and then four times, in the work
  that is left. `nrfmicro` 11 open over three orders with and without, `corne-cherry`
  130, `buspirate5-rev10` 11, and `lumenpnp-mobo` 2 for 1.

### What this does not show

- A way out of the pads of a QFN of 0.4 mm on a board of two layers, still.
  `corne-cherry` has 43 to 50 open at 40 million nodes against 3 for cadlab's router:
  thirty of them are pads of its two QFNs and its USB receptacles with the tracks and
  vias of four other nets round them. A chain for such a pad is five nets long before it
  starts, and the fifth has nowhere to go: seven in eight of the chains tried are undone.
  What its neighbours need is to have left the row another way from the start.
- Searches that end at their limit of nodes on a wide board: `sweep-v2.2`'s ground, and
  what `tinytapeout-demo` has left. A chain for them is not found.
- A run with a deadline on a machine that is free.
- What the chains would make of the budget that they leave: on `corne-cherry` a run ends
  with half of its 40 million nodes, every open connection tried four times.
- Vias and track. Not looked into here.

## PCB routing negotiated

What was changed after [the release above](#pcb-routing-settled), 0.0.11, for the same
host, cadlab, and the first of what that section does not show: the pads of rows of fine
pitch that the routes round them shut in ([what and why](architecture.md#decisions)).
Measured on 2026-11-17, each run with one thread, six runs at a time, on a machine that
other work shared with a load of 5 to 9 on its 64 threads.

### How

Through cadlab's adapter, with the rows planned on boards of two layers
(`RouteOptions::row_fanout`) as its `master`, commit `1a70ba8`, has them for 0.0.11, and
left off for this, which has `RouteOptions::negotiate`, its default:

```text
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_WORK=40000000 [CADLAB_BENCH_SEED=2|3] route_bench --corpus <board>
RAYON_NUM_THREADS=1 CADLAB_BENCH_ROUTER=guide CADLAB_BENCH_VERBOSE=1 CADLAB_BENCH_BUDGET_MS=60000 [CADLAB_BENCH_SEED=2..6] route_bench --corpus <board>
```

After a run that its budget of work stopped the adapter makes no second, and cadlab
counts what the one run leaves open on the board that it fills again. With a deadline
the adapter gives a first run seven tenths of it and a second, on the board filled again
with the first run's routes as copper that does not move, the rest.

### Results

cadlab's corpus with a budget of 40 million nodes, vias kept out of pads, in three orders
of the nets, no seed and seeds 2 and 3. Connections left open as cadlab counts them,
before → after; vias, track and the seconds the call took are of the first order:

| Board | Layers | Open | Vias | Track, mm | Seconds |
| --- | --- | --- | --- | --- | --- |
| `nrfmicro` | 2 | 3, 5, 3 → 2, 4, 3 | 49 → 35 | 500 → 521 | 6 → 11 |
| `sweep-v2.2` | 2 | 6, 7, 6 → 6, 7, 6 | 42 → 42 | 2231 → 2231 | 53 → 56 |
| `corne-cherry` | 2 | 40, 45, 42 → 40, 42, 36 | 475 → 483 | 5912 → 6018 | 35 → 40 |
| `tinytapeout-demo` | 4 | 14, 19, 5 → 16, 19, 5 | 608 → 604 | 5685 → 5655 | 46 → 47 |
| `buspirate5-rev10` | 4 | 0, 6, 5 → 1, 1, 7 | 502 → 476 | 3818 → 3756 | 37 → 38 |
| `lumenpnp-mobo` | 4 | 0, 1, 0 → 0, 1, 0 | 713 → 713 | 6544 → 6544 | 32 → 32 |
| `cynthion` | 4 | 21, 17, 25 → 13, 17, 25 | 613 → 586 | 3618 → 3576 | 44 → 49 |
| `glasgow-revC3` | 4 | 36, 35, 36 → 36, 37, 36 | 563 → 563 | 3937 → 3937 | 73 → 72 |
| `glasgow-revD1` | 6 | 170, 145, 139 → 170, 145, 139 | 1067 → 1067 | 7616 → 7616 | 48 → 48 |
| All, three orders | | 831 → 814 | | | |

`buspirate-flash-sop`, `buspirate-rs232` and `lumenpnp-ringlight` are complete before and
after, in under a second. No run has an error of cadlab's check.

With a deadline of 60 s, where the first run's routes come back in time for a second,
`corne-cherry` has 155 connections open over six orders of the nets (17, 19, 35, 25, 29
and 30) with the rows planned and 160 without (36, 30, 25, 25, 23 and 21), where 0.0.11
with the rows has 100 over the first three for 71 and 91, and `nrfmicro` 18 and 19 over
six for 11 over three before and 9 after. So the rows planned make no more with the
negotiations, and are left off.

What a negotiation is, on `nrfmicro`'s `DATA+`, whose pads in its USB receptacle have
those of `DATA-` between them, in a build with the negotiations after the last pass: its
route crosses four nets, twelve nets take part over fourteen rounds, with three to seven
nets in each other's way until the price is 88, two at 140 and none at 225, in 235,000
nodes, and the board was complete, where every chain for it was undone. As released,
with the negotiations after the first pass, that connection's is undone in each of the
three orders, on a board that the first pass left otherwise. In a first run of `corne-cherry` with 42 s,
the first pass ends with 390 of 432 connections after 7.6 million nodes, 19 to 35
negotiations are tried in the 30 million that are left, 7 to 15 of them kept, and they
make 8 to 27 connections: 408 to 417 when the run is cut. One that is kept costs
860,000 nodes and makes 1.4 connections, and one that is undone half as much again;
about half of those kept end with the price at its most and what is still shared given
up. With four times the deadline the first run ends on its own with 415 or 416: what is
left is not for want of time.

On the boards of four and six layers the first run of a minute ends in its passes, or
its open pads are balls of an array with nothing round them that moves:
`glasgow-revC3` had 34 negotiations for one kept. A negotiation that finds its pad shut
in by what nothing shares room with ends after a look of 2,000 nodes, and after eight
undone in a row a settling has no more.

The curated boards, routed with no budget, are as they were, 1150 of 1151 connections
with 712 vias: a run with neither a budget nor a deadline negotiates nothing.

### What was tried and not kept

At 60 s through the adapter, connections open as cadlab counts them, `corne-cherry`
unless said.

- *The nets in the way routed again whole*, from nothing, as congestion is negotiated
  on an FPGA: after one net of `nrfmicro` had crossed four, twenty of its forty-one nets
  were in each other's way, and no fewer at any price in twenty-four rounds. A net
  routed again from nothing comes back by another way, across others that had nothing
  to do with it.
- *Joined up again from the ends that a crossing leaves*, with nothing more taken out:
  69 over three orders for 62 to 68 with twelve or six pitches round the place taken
  out, and tracks that keep every bend they ever made.
- *A window of twelve pitches* to be joined up again within, for twenty-four: 217 over
  six orders for 159.
- *The rounds ended once the same nets are in each other's way twice in a row* at a price
  over twenty: 82 over three orders for 62. Two nets that cross each other at any price
  do move, a few rounds on, when the history at that place has made another dearer.
- *20,000 nodes for a search* in place of 60,000: 84 for 62. *A price that starts at one
  and rises by 2.2*: 83. *Forty rounds*: 77.
- *Two more negotiations* for a connection whose first was undone, each cutting by
  another distance, six and twenty-four pitches, and with the nets' turns in the other
  order: 166 over six orders for 159, `nrfmicro` 16 for 18.
- *A negotiation for each net with pads cut off its fill*, the pieces as terminals of
  their own, before the others: 174 over six orders for 161, `tinytapeout-demo`,
  `lumenpnp-mobo` and `sweep-v2.2` the same.
- *The whole board negotiated from nothing* in place of the first pass, every net across
  every other: after 64 rounds and 36 million nodes, 394 of 432 connections, with one to
  nineteen nets still in each other's way from one round to the next; the first pass has 390
  after 3.4 million.
- *The negotiations that were undone tried again after each pass*, on the board as the
  pass left it: 152 over six orders for 155.
- *A limit of 100,000 or 300,000 nodes on the first pass's searches across routes*, for
  `sweep-v2.2`'s three long nets to come to their negotiations sooner: the same 6, 7 and
  6. Its first pass ends after 12 s as it is, and its settling goes on for 26 s more.
- *Chains after every pass* as the negotiations are: `glasgow-revC3` 112 open over
  three orders at 40 million nodes for 109 with the chains after the last pass alone,
  as before, and 107 with 0.0.11.

### What this does not show

- A board of two layers with QFNs of 0.4 mm routed whole: `corne-cherry` has 21 to 36
  open at 60 s against 3 for cadlab's router. Of those in an order, some fifteen are
  pads of its two QFNs, each with a fan of a dozen tracks beside it that would have to
  move over by one, which thirty-two nets and twenty-four rounds do not do; five to
  ten are pads that routes cut off the ground's pour, found by the host's fill and
  given to a second run in which nothing moves; and one or two are the `D+` of a USB
  receptacle, whose pads have those of `D-` between them.
- A second run that goes on from the first run's routes, in place of one on a board
  where they are fixed.
- `sweep-v2.2`, which is as it was, and not for the reason given before. Its ground,
  `raw` and `vcc` each have terminals in both halves of the board. A negotiation's
  search for them, with every route shared at half its length's price, expands its
  60,000 nodes and reaches nothing, and with 800,000 it ends after 380,000 to 650,000
  with nowhere left to go: what stands between the halves for these three nets is not
  other nets' routes but something that does not move, at the width they are routed at.
  What that is was not found here.
- Vias and track, and a board of six layers, whose minute ends in its first pass.

## PCB routing of a board in two parts

What was changed after [the release above](#pcb-routing-negotiated), 0.0.12, for the same
host, cadlab, and the last of what that section does not show: `sweep-v2.2`, a keyboard
in two halves on one board that only its schematic joins. Measured on 2026-11-17, each
run with one thread, four to six runs at a time, on a machine that other work shared with
a load of 6 to 15 on its 64 threads.

### What

Its ground, `raw` and `vcc` each have pads in both halves, and between the halves a row
of holes that no track passes. A net's tree grew from its first terminal alone, so such a
net joined the pads of the half with its first terminal and left every pad of the other
on its own: 6, 7 and 6 connections open, where three are open on any routing of the
board, one for each of the three nets. In a run with a budget or a deadline, a route
that keeps what it reaches (`RouteOptions::keep_partial`, a net with a plane, or one of
many pads) and leaves two to eight terminals out now begins another tree from one of
them, twice at most, so that the pads that nothing joins to the first are joined among
themselves; the net's pieces are counted from its copper, as those of a net that was cut
are, and a later turn joins them if it can.

It does so in every search but one: the search that a net has with nothing moved, in the
first pass, before its turn to cross other nets' routes. What that search leaves is
copper that waits for the net's next turn, and pads joined among themselves that the
first terminal reaches none of are then in the way of the nets still to come.

### Results

Through cadlab's adapter as in the section above. Connections left open as cadlab counts
them, before → after, vias and track of the first order.

With a budget of 40 million nodes, one run, three orders of the nets:

| Board | Layers | Open | Vias | Track, mm |
| --- | --- | --- | --- | --- |
| `nrfmicro` | 2 | 2, 4, 3 → 5, 2, 3 | 35 → 44 | 521 → 494 |
| `sweep-v2.2` | 2 | 6, 7, 6 → 5, 6, 5 | 42 → 42 | 2231 → 2242 |
| `corne-cherry` | 2 | 40, 42, 36 → 38, 38, 32 | 483 → 487 | 6018 → 6050 |
| `tinytapeout-demo` | 4 | 16, 19, 5 → 16, 19, 5 | 604 → 604 | 5655 → 5655 |
| `buspirate5-rev10` | 4 | 1, 1, 7 → 0, 5, 7 | 476 → 488 | 3756 → 3776 |
| `lumenpnp-mobo` | 4 | 0, 1, 0 → 0, 1, 0 | 713 → 712 | 6544 → 6538 |
| `cynthion` | 4 | 13, 17, 25 → 14, 17, 25 | 586 → 583 | 3576 → 3540 |
| `glasgow-revC3` | 4 | 36, 37, 36 → 32, 35, 37 | 563 → 563 | 3937 → 3961 |
| `glasgow-revD1` | 6 | 170, 145, 139 → 170, 145, 139 | 1067 → 1067 | 7616 → 7616 |
| All, three orders | | 814 → 801 | | |

`sweep-v2.2` is cut by that budget in its first pass, whose searches for the three
connections that no route makes cost it most of its nodes.

With a deadline of 60 s, a first run with seven tenths of it and a second on the board
filled again, four orders of the nets (no seed, 2, 3 and 4):

| Board | Open before | Open after |
| --- | --- | --- |
| `nrfmicro` | 2, 4, 3, 3 | 5, 2, 3, 3 |
| `sweep-v2.2` | 6, 7, 6, 7 | 3, 3, 3, 3 |
| `corne-cherry` | 34, 28, 26, 27 | 33, 27, 21, 26 |
| `tinytapeout-demo` | 9, 6, 5, 18 | 9, 7, 5, 16 |
| `buspirate5-rev10` | 1, 7, 7, 4 | 0, 5, 4, 0 |
| `lumenpnp-mobo` | 0, 1, 0, 1 | 0, 1, 0, 1 |
| `cynthion` | 17, 15, 25, 22 | 14, 15, 23, 15 |
| `glasgow-revC3` | 36, 36, 41, 34 | 33, 31, 35, 34 |
| `glasgow-revD1` | 162, 137, 124, 121 | 162, 136, 124, 121 |
| All, four orders | 982 | 923 |

`sweep-v2.2` has the three that every routing of it has. The curated boards, routed with
no budget, are as they were, 1150 of 1151 connections with 712 vias: a run with neither
a budget nor a deadline begins no second tree.

### What was tried and not kept

At 60 s through the adapter, `tinytapeout-demo` over six orders of the nets, which has
59 connections open with 0.0.12 (9, 6, 5, 18, 9 and 12).

- *Another tree in every search that keeps what it reaches*: 91 (14, 20, 18, 18, 8 and
  13), with `sweep-v2.2` at 3, 3 and 3. The first run then ends with up to sixteen
  connections open for six, of nets that had room before.
- *Not for the nets with a plane*, whose pads each wait for a stub of their own: 81.
  *Not for the pads that routes cut off a fill*, joined again after routing: 85. *For
  neither*: 81.
- *Only in the searches that may cross other nets' routes*, none in a negotiation or a
  chain's search with nothing moved: 60, with `sweep-v2.2` at 4, 6 and 4.
- *In every search but the first pass's with nothing moved*, as released: 60, with
  `sweep-v2.2` at 3, 3 and 4 in those runs and 3 in each of the four of the table.

### What this does not show

- `glasgow-revD1` with the first variant had 373 to 408 open over three orders for 421:
  on a board of six layers whose first pass does not end in a minute, pads joined among
  themselves early are connections made. As released it is as it was.
- Whether a net left in two trees should be joined across other nets' routes first: a
  net's later turns go on from its pieces as they do for a net that was cut.

## PCB routing from an earlier run's routes

What was changed after [the release above](#pcb-routing-of-a-board-in-two-parts), 0.0.13,
for the same host, cadlab: a run that goes on from the routes of an earlier one and may
move them (`pcb::route_from`), and a negotiation for each place where several pads are
open near each other (`RouteOptions::negotiate_places`, off by default). Measured on
2026-11-17, each run with one thread, four to six runs at a time, on a machine that other
work shared with a load of 6 to 15 on its 64 threads.

### What

cadlab fills the zones itself. With a deadline its adapter gave a first run seven tenths
of it, filled the board with that run's routes, and gave the nets left open to a second
run on a board where those routes were copper that does not move: a pad that the fill no
longer reached had a stub if there was room for one as the routes lay, and a pad that the
first run left shut in stayed so.

`route_from` takes the tracks and vias of an earlier run beside the design. They are the
routes of their nets from the start: a net that they join whole has no turn until another
net moves it, one that they leave in pieces is completed from them, with its turns as a
net routed for the first time has them (nothing moved until every net has had one, across
other nets' routes after), and what the run returns holds them as they are then. A track
that is not as wide as its net is routed, an arc, a via of another size or a net with
nothing to join stays as copper of the board, and `Routed::taken_back` says which nets
were taken. The stubs of a net with a plane end where the fill was: each end that a piece
of the fill still holds is a place of that piece's terminal, as the places looked for
beside a pad are. A net that the run does not move is not cleaned up again: its routes
are returned as they were given.

The adapter now has up to three runs: the first, the second on the fixed board as
before, and a third with `route_from` and every net left open, in what time the second
leaves, kept if cadlab's count of what is open is less for it.

### Results

Through cadlab's adapter, 60 s, four orders of the nets (no seed, 2, 3 and 4), connections
left open as cadlab counts them, 0.0.13 behind the adapter with two runs and this release's
source behind the one with three:

| Board | Open before | Open after |
| --- | --- | --- |
| `nrfmicro` | 5, 2, 3, 3 | 5, 2, 3, 3 |
| `sweep-v2.2` | 3, 3, 3, 3 | 3, 3, 3, 3 |
| `corne-cherry` | 33, 27, 21, 26 | 15, 27, 19, 26 |
| `tinytapeout-demo` | 9, 7, 5, 16 | 9, 7, 1, 16 |
| `buspirate5-rev10` | 0, 5, 4, 0 | 0, 1, 0, 0 |
| `lumenpnp-mobo` | 0, 1, 0, 1 | 0, 1, 0, 0 |
| `cynthion` | 14, 15, 23, 15 | 14, 15, 20, 15 |
| `glasgow-revC3` | 33, 31, 35, 34 | 33, 31, 35, 34 |
| `glasgow-revD1` | 162, 136, 124, 121 | 162, 136, 124, 124 |
| All, four orders | 923 | 890 |

No board has more open in an order but `glasgow-revD1` in one, by three, in its first run,
which the two builds share: a third run is only kept if it leaves less open than the two
before it. With a budget of 40 million nodes a run that its budget stops is the same to
the track (`corne-cherry` 38, 38 and 32, `cynthion` 14, 17 and 25, as before), and one
that ends within it goes on: `tinytapeout-demo` 16, 19 and 1 for 16, 19 and 5.

The negotiations for places, with `corne-cherry`'s first run given 168 s of a budget of
four minutes, which it does not use up: 9 and 11 connections open in two orders with them
for 13 and 16 without, and 8 in a third run of the second order. In such a run a place
of five to twelve pads has twenty to thirty-two nets, its negotiation costs three to
eight million nodes, and four of five are kept, for 24 of the 55 connections and pieces
of fill that the first pass leaves open.

### What was tried and not kept

At 60 s through the adapter, four orders, the nine boards above (923 open with 0.0.13).

- *The second run itself from the first run's routes*, in place of a run on the fixed
  board: `sweep-v2.2` 15 to 21 over the four orders for 12, as its second run then has
  searches across routes for the three connections that no route makes, and is cut before
  it has joined what a run on the fixed board joins; `lumenpnp-mobo` 5 for 2.
- *Every net cleaned up again* at the end of such a run: `lumenpnp-mobo` 5 to 10 over
  four orders for 1 without. A route pulled tight lies where the host's fill has copper, and
  the fill after it is cut where it was whole.
- *The routes of nets with a plane left as copper of the board*, only the others taken
  back: `corne-cherry` 114 for 88 to 92 with them taken back, whose zones are tracks as
  wide as a pad; 926 on the nine boards.
- *The nets completed as nets that are mended are*, near where they were and across
  other nets' routes at once: `sweep-v2.2` 21 for 16 with the turns of a first pass.
- *The negotiations for places in a run of 60 s*: `corne-cherry` 71, 84 and 87 over four
  orders in three builds for 87 without, `cynthion` 75 for 64, `sweep-v2.2` 14 for 12,
  `nrfmicro` 10 for 13: three or four places take the nodes of the first run, and what
  they make the negotiations for single connections made before.
- *A place's nets kept to its window*, each joined up again within twelve pitches of it:
  the nets with a pad open have their other pad far off, stay open through every round,
  and no negotiation is kept. *As many nets as a negotiation for one connection may
  have*, sixteen more than it begins with: ended in the second round, as nets routed
  over the board run into others.

### What this does not show

- `corne-cherry` level with cadlab's own router, which has 0 to 4 open in 35 s: 15 to
  27 at 60 s, 9 to 11 with four minutes and the places.
- A negotiation for a place that a run of a minute can pay for.
- The pads of a USB receptacle, a board of six layers whose first pass does not end in a
  minute, vias and track: as in the sections above.

## PCB routing of two nets that cross in a row

What was changed after [the release above](#pcb-routing-from-an-earlier-runs-routes),
0.0.14, for the same host, cadlab: the crossing of two nets whose pads take turns along a
row of fine pitch is planned before anything is routed, where it has no room on the row's
layer (`RouteOptions::crossings`, on by default), and cleanup takes out the hops that
its search for a whole net with dearer vias leaves. Measured on 2026-11-17 and 18, each
run with one thread, five to twelve runs at a time, on a machine that other work shared.

### What

The four data pads of a USB receptacle are in the order D+ D− D+ D− at 0.5 mm, with the
board's edge behind them. On their layer one of the two nets joins its pads by a track
round the pad of the other between them, and that pad then needs a via at its end, which
is in the way of the tracks out of the pads beside it unless both step aside. Where the
track round has room, rip-up and the negotiations find that. On `nrfmicro` a diode's pad
is 1.9 mm in front of the row: there is no room for a via and a track round it, the two
nets have to cross under the board, and no run found that. Every connection that
`nrfmicro` had open in every order, 2 to 5 of 85, was of that place: a pad of D+ between
two of D− shut in, or the nets pushed out of it.

`pcb::cross` looks at each row of fine pitch for a pad between two pads of another net
with a pad of its own net two along. It plans a via straight out of that pad, at the
least depth at which the via is legal and the two pads beside it have a way past it:
straight on, a step aside at 45° by what the via needs, straight on to the via's depth.
If a track of the other net then fits round the via from one way to the other, the
crossing is left to the routing. If not, the net's other pad gets a via of its own, clear
of the way between the two, and a track on the layer furthest from the row's joins the two
vias; the pads further along that a way steps towards get a step aside of their own, each
by less, until one has room straight on. All of it is fixed copper of its nets from then
on, as the escapes of an array are (`Stats::crossings` counts the vias).

Cleanup routes each net with vias again with vias ten times as dear, within a limit of
nodes, and keeps the result if it has fewer. A net of two pads with a dozen vias is where
that search reaches its limit and nothing changes. `fewer_hops` then takes each run of
tracks between two vias that both go back to the layer the net came on, and looks for
tracks on that layer between the two, within sixteen pitches of them and 20,000 nodes,
with no via to place: found, and no longer than the run by more than a via's price in
that search, they replace the run and its two vias.

### Results

Through cadlab's adapter as it was for 0.0.14, 60 s, four orders of the nets (no seed, 2,
3, 4), connections open as cadlab counts them on the board filled again, 0.0.14 and this
release measured one after the other:

| Board | 0.0.14 | With this | Vias, first order |
| --- | --- | --- | --- |
| `nrfmicro` | 5, 2, 3, 3 | 1, 1, 1, 3 | 44 → 55 |
| `sweep-v2.2` | 3, 3, 3, 3 | 3, 3, 3, 3 | 48 → 48 |
| `corne-cherry` | 15, 27, 19, 26 | 15, 22, 19, 23 | 529 → 526 |
| `tinytapeout-demo` | 9, 7, 1, 16 | 9, 7, 1, 16 | 605 → 593 |
| `buspirate5-rev10` | 0, 2, 3, 0 | 0, 1, 1, 0 | 484 → 482 |
| `lumenpnp-mobo` | 0, 1, 0, 0 | 0, 0, 0, 0 | 712 → 706 |
| `cynthion` | 15, 15, 25, 17 | 14, 15, 25, 15 | 512 → 515 |
| `glasgow-revC3` | 33, 31, 35, 34 | 33, 31, 35, 34 | 560 → 558 |
| `glasgow-revD1` | 162, 136, 124, 124 | 162, 137, 124, 120 | 1085 → 1085 |
| All twelve | 902 | 877 | |

The three small boards have nothing open in either. `nrfmicro` is the board the plan is
for: one crossing, two vias. `corne-cherry`'s two receptacles, `buspirate5-rev10`'s and
`tinytapeout-demo`'s have room round the via and get no plan; what those boards differ by
is the hops, and one run of a board that differs by as much from itself.

At 40 million nodes, one run of guide, three orders, the same on any machine:

| Board | 0.0.14 | With this |
| --- | --- | --- |
| `nrfmicro` | 5, 2, 3 | 1, 1, 1 |
| `sweep-v2.2` | 5, 6, 5 | 5, 6, 5 |
| `corne-cherry` | 38, 38, 32 | 39, 41, 32 |
| `tinytapeout-demo` | 16, 19, 1 | 16, 19, 1 |
| `buspirate5-rev10` | 0, 5, 7 | 0, 5, 7 |
| `lumenpnp-mobo` | 0, 1, 0 | 0, 0, 0 |
| `cynthion` | 14, 17, 25 | 14, 17, 25 |
| `glasgow-revC3` | 32, 35, 37 | 32, 35, 37 |
| `glasgow-revD1` | 170, 145, 139 | 170, 145, 139 |
| The nine | 797 | 793 |

Vias there: `lumenpnp-mobo` 712, 699 and 738 → 706, 682 and 719; `tinytapeout-demo` 604,
646 and 488 → 600, 640 and 472. The hops are a few vias in a hundred. Of
`lumenpnp-mobo`'s 712, 221 are of its ground and 71 of its 3.3 V, a stub through a via
from each pad to the plane inside, where cadlab's own router has 25 and 52: its fill of
the pours on the outer layers joins most of those pads without one. That is the host's to
tell, as it is the host's fill: see what was not kept.

The curated boards, routed with no budget, plan no crossing: 1150 of 1151 connections
with 712 vias, as before, and 11 mm more track on `GB-LIVE32` (2284 for 2273).

### What was tried and not kept

- *Every crossing planned*, those with room round the via too, the via, the two ways
  and the track round it as fixed copper: at 60 s over six orders `nrfmicro` 11 open for
  20, `corne-cherry` 154 for 137, `tinytapeout-demo` 67 for 54, `buspirate5-rev10` 7 for
  6. The copper is in the way of the pads beside it, the receptacle's CC pads on
  `corne-cherry`, which had room when nothing was fixed.
- *The crossing as routes of their nets*, which the nets go on from and rip-up may take
  out, as the stubs of `row_fanout` are: `nrfmicro` 3, 3, 2 and 4 open for 5, 2, 3 and
  3. The via is gone the first time a net crosses the place.
- *The via and the two ways alone*, with no via for the net's other pad: 2, 3, 2 and 4.
  That pad is then shut in between the way beside it and the pad of no net next to it.
- *The parts of a board that its edge keeps apart told from a flood of its area*, for the
  connections of `sweep-v2.2` that no route makes not to be searched for: its outline is
  one, and its two halves are one area. What keeps them apart is not the edge. Not
  released.
- *The pours of outer layers planned on too*, where a net has a plane inside, so that a
  pad on its net's pour needs no stub: at 40 million nodes over three orders
  `lumenpnp-mobo` 576, 584 and 554 vias for 712, 699 and 738 with 3 open in each order
  for 0, 1 and 0, `tinytapeout-demo` 153 open for 36, `buspirate5-rev10` 113 for 12,
  `cynthion` 130 for 56, `glasgow-revD1` 837 for 454. Routes cut an outer pour, and the
  pads counted as joined by it are then not.
- *The negotiations for places made cheaper*, at 60 s over four orders through an adapter
  that asks for them: as they are `corne-cherry` 89 open for 80 without and `cynthion`
  74 for 70; with the price rising by 1.6 a round once six nets or fewer are in each
  other's way, 15,000 nodes for the search of a net that is whole, 2.5 million nodes a
  place and the two largest places alone, 94 and 78; with the faster price from ten
  nets, 4 million nodes a place and two places, 89 and 71. What a place costs is not
  what keeps it from gaining in a minute: its rounds take the time of the run that moves
  routes after the fill, which makes more.
- *The hops before the search for the whole net*: `GB-LIVE32` 195 vias for 189, the
  curated boards 718 for 712. After it they are 712.

### What this does not show

- `corne-cherry` level with cadlab's own router: 15 to 23 open at 60 s for its 0 to 4.
  The pads of its two QFNs of 0.4 mm are what is open, as before, and nothing here is
  for them.
- The connection that `nrfmicro` still has open in each order, of one of the two data
  nets or a net beside them: the crossing is made, and the room behind the receptacle is
  what is short. Its run ends after 20 s of 60.
- Vias as few as a router that plans on the host's fill has them, a board of six layers
  whose first pass does not end in a minute, `sweep-v2.2` at its deadline: not touched.

## PCB routing with room for spokes

What was changed after [the release above](#pcb-routing-of-two-nets-that-cross-in-a-row),
0.0.15, for the same host, cadlab: a net that is completed puts no via beside one it has,
the room of a thermal relief's spokes can cost other nets' tracks
(`RouteOptions::spoke_cost`, off by default), and the connections of a board that its own
copper keeps apart are found once and told (`Cause::Apart`). Measured on 2026-11-18, each
run with one thread, four to nine runs at a time, on a machine that other work shared;
the counts of pads are cadlab's, from its fill of each routed board (`board::thermal`).

### What

**A via on a via.** One run in four of `buspirate5-rev10` at 60 s, in one order of its
nets, ended with two vias of its ground 0.18 mm apart, and cadlab's check took the piece
out. A net that is completed is searched for from the copper it keeps, and what the
search finds is added to it. In a negotiation that copper is out of the world while the
net is looked for, so nothing was at the place of a via it kept. The net as it is
completed now has the vias of its copper, and its search keeps its own from them as from
those of its path.

**Spokes.** A pad in a pour of its net is joined by a few spokes across a gap. A track of
another net beside the pad takes one away, and after a run more pads had one spoke or
none than on the boards as their projects route them. With `spoke_cost`, each spoke of
each pad that a zone of its net joins through a relief is marked on a grid, by layer:
from the pad's edge across its gap and a pitch into the zone, as wide as the spoke and
what a track beside it needs. A track of another net that ends in a marked cell costs
that many times its length more.

**Apart.** `sweep-v2.2` is two halves of a keyboard on one board, joined by the schematic
alone: three nets have pads on both, and no route. Each search for one of them went over
a half of the board to find that out, in every pass and every chain. The first time a
search for a net fails in a run with a budget, the net is now looked for on the board
with none of the run's routes on it; the connections that has no way for are counted, a
net that has all its others has no more turns, and they are reported as `Cause::Apart`.

### Results

The twelve corpus boards through cadlab's adapter at 60 s, two orders of the nets, with
`spoke_cost` 4 as cadlab sets it. Pads in pours by the necks that reach them, pads short
of the two that cadlab asks for, and connections open:

| | necks 0 / 1 / 2 / 3 / 4+ | short of two | open |
| --- | --- | --- | --- |
| 0.0.15, first order | 5 / 390 / 360 / 238 / 416 | 195 | 237 |
| 0.0.15, second order | 8 / 368 / 353 / 263 / 405 | 169 | 215 |
| this, first order | 4 / 328 / 329 / 268 / 462 | 127 | 231 |
| this, second order | 1 / 325 / 333 / 268 / 458 | 127 | 177 |

By board in the first order, short of two before and after: `buspirate5-rev10` 62 and
43, `glasgow-revD1` 69 and 52, `tinytapeout-demo` 33 and 24, `lumenpnp-mobo` 20 and 0,
`corne-cherry` 5 and 4, `sweep-v2.2` 3 and 1, `cynthion` 1 and 3, the other five 2 and
0. Open, before and after, over the two orders: `corne-cherry` 15 and 22 for 26 and 17,
`tinytapeout-demo` 9 and 7 for 7 and 5, `cynthion` 14 and 15 for 14 and 10,
`glasgow-revC3` 33 and 31 for 34 and 31, `glasgow-revD1` 162 and 136 for 145 and 111,
`nrfmicro` 1 and 1 for 2 and 0, `buspirate5-rev10` 0 and 0 for both, `lumenpnp-mobo`
0 and 0 for both, `sweep-v2.2` 3 and 3 for both. None of these runs had copper taken out
by cadlab's check.

Those spokes were worked out here, from the zones and what the pads say. cadlab's fill
has since changed: it fits a spoke's width to its pad, places spokes in other directions
where the first do not fit, and has a connection by layer. A host gives the spokes as its
fill makes them (`Zones::spokes`), and with cadlab's given so, on its new fill, the same
two orders have 97 and 81 pads short of two for 138 and 128 with 0.0.15, and 209 and 205
connections open for 238 and 218.

`sweep-v2.2` over four orders at 60 s: 3 open in each, where 0.0.15 has 3, 4, 4 and 4.
Two of the three are told as apart; the third is of its ground, which has a pour and is
not looked at (below). Its first run ends after 25 s of 42 with those two nets out of
its passes, and the rest of the minute goes to the runs for the ground.

At 40 million nodes over three orders, the nine boards that a budget cuts, 0.0.15 with
the fix for the via alone and this with the price on: 782 open and 733 (`glasgow-revD1`
440 and 389, `corne-cherry` 113 and 105, `sweep-v2.2` 16 and 9, `buspirate5-rev10` 12 and
21, `tinytapeout-demo` 35 and 40, `cynthion` 56 and 58, `glasgow-revC3` 104 and 106,
`nrfmicro` 6 and 5, `lumenpnp-mobo` 0 and 0).

The curated boards route as before, 1150 of 1151 connections with 712 vias, with the
price off as it is by default; with it at 4 they have the same, and two boards a
millimetre more track.

### What was tried

- *A via beside the net's own hole refused by the world*, whatever the search: the
  holes of a net's own routes as what may not be moved. Six orders at 40 million nodes:
  `nrfmicro` 22 open for 1, `corne-cherry` 299 for 231, `buspirate5-rev10` 40 for 15. A
  negotiation routes a net across its own old route, which then goes, and it could no
  longer. With the vias told by the net as it is completed, the same runs have 4, 234
  and 17.
- *The price by its size*, with a via in a marked place costing as many more vias: at
  2, 4 and 12 the pads short of two were 135 and 129, 117 and 122, 121 and 111 over the
  two orders, for 200 and 163 with none; open 260 and 229, 240 and 250, 289 and 305, for
  255 and 212. Past 4 it costs connections and saves no more pads.
- *A price on vias too.* Four orders of three boards at 60 s, open and short of two
  summed: with it `lumenpnp-mobo` 3 open (0, 3, 0, 0) and 0 short, `buspirate5-rev10` 7
  and 179, `tinytapeout-demo` 27 and 79; with tracks alone 0 and 4, 1 and 177, 48 and
  102; with neither 0 and 67, 8 and 234, 33 and 113. `lumenpnp-mobo` had no connection
  open in any run before, and has none with tracks alone. `tinytapeout-demo` is 0 to 19
  open in these runs whatever is asked.
- *The spokes marked to the end of their gap only*, with no pitch into the pour: 13
  short on `lumenpnp-mobo` for 0, and `buspirate5-rev10` 16 open over four orders.
- *Ways out that stay for the pads of a row that the routes leave shut in*, in a run
  that goes on from an earlier run's routes: a straight track out of an end of the pad
  to a via, as fixed copper, at the place in the way of the fewest routes, which are
  then cut and joined up again around it. `corne-cherry` at 60 s over four orders: 102
  open for 89; at 120 s 84 for 84, and the run that plans 17 of them has 87 open after
  40 s where it began with 22. Only at places in no route's way: 90 for 89, with one to
  five pads a run. The pads of its two QFNs that stay open are those of nets taken out
  and not routed again, by other nets that are locked by then, and a via fixed in front
  of a row shuts the pads beside it for good. cadlab's own router plans no way out for
  the first of the two parts at all, whose pads are on its grid, and straight stubs with
  no via for two rows of the second: what it does that this does not is to route every
  net across every other at a price, on one grid, until none is in another's way.
- *More rounds of cleanup, and a shorter way for each net*: every net routed again as
  routing does, the others where they are, kept if it has no more vias and is 3 %
  shorter, for up to four rounds while a round gains. `lumenpnp-mobo` 657 and 680 vias
  for 706 and 510, in 27 s of cleanup for 5, which is the time cadlab takes the needless
  stubs out in; `tinytapeout-demo` 601 and 631 for 593 and 616; track 1 % shorter on two
  boards and longer on two.
- *Dearer vias.* At 2 and 3 mm of track a via, for 1: 10 to 25 % fewer vias at 40
  million nodes (`lumenpnp-mobo` 491, 467 and 461 at 3 for 706, 510 and 571), and
  `buspirate5-rev10` 28 and 19 open for 12, `glasgow-revD1` 602 and 601 for 440,
  `cynthion` 46 and 69 for 56, `corne-cherry` 89 and 93 for 113. On the three boards of
  two layers at 60 s over four orders: `corne-cherry` 75 and 83 for 89, `nrfmicro` 5 and
  2 for 5, `sweep-v2.2` 16 and 12 for 15. Left at 1.
- *A pass of rip-up given four times as long with no gain before it ends*: 232 open for
  222 on five boards at 40 million nodes. *A net ripped up eight times before it is
  locked*, for four: `buspirate5-rev10` 33 for 12, `cynthion` 65 for 56.
- *Nets with a plane looked for on the bare board too*: `sweep-v2.2` 4 and 5 open for
  3. Their terminals there are places on a fill that no route has cut, and the count of
  what is apart comes out higher than what the net is left with.
- *A net that goes on at the width its tracks have*, those of the board and of an earlier
  run: in a run on a board with routes beside its pads no straight track leaves a pad at
  any width, so a net that the first run necked is back at its own width, which between
  the pads of a 0.4 mm QFN leaves none. `tinytapeout-demo` at 60 s over four orders: 6,
  10, 0 and 14 open for 7, 7, 2 and 14; `buspirate5-rev10` 4 for 4. The pads of its supply
  that this was for are joined on cadlab's new fill without it. Not released.
- *The finest lattice alone as what says that two pads are apart*: `sweep-v2.2`'s halves
  are more than 4 million nodes on it, and nothing was told. It has the say where it
  ends within that; where it does not, the lattice of the pass has.

### What this does not show

- `corne-cherry` level with cadlab's own router, 15 to 26 open in these runs for its 0
  to 4, nor `nrfmicro`'s last connection, which is of a data net across the board with
  the nets beside it settled in its way: both are what one-net-at-a-time rip-up leaves,
  and nothing here changes that.
- Half as many vias as now. Of `lumenpnp-mobo`'s 706, 309 are stubs to planes and some
  400 are of its other nets, against 344 in all for cadlab's router, which prices a via
  at 2 mm and routes every net against every other; routing a net again with dearer
  vias, the others where they are, takes few away.
- Spokes on every pad. 127 pads of 2350 are still short of two, most of them on two
  boards whose pads in pours are close together.
- The time of `sweep-v2.2`, which still ends at its deadline for the one connection of
  its ground that has no way.
- Rule areas with a track width of their own: not touched.
- A board of six layers in a minute. `glasgow-revD1`'s first pass was timed by net, with
  counters in the loop of the passes (this machine lets no process be sampled): of 42 s
  of routing at 40 million nodes, 27 s are in the searches of its 561 nets, the longest
  2.4 s and 25 of them over 0.2 s, and 15 s are outside any search, in what a turn does
  before and after it: the routes kept for a run that is stopped, the pieces of the nets
  that a route cut, the count of connections made at every turn. No net stands out; the
  third of the pass outside the searches is where to look first.

## PCB routing of every net at once

What was changed after [the release above](#pcb-routing-with-room-for-spokes), 0.0.16, for
the same host, cadlab: `RouteOptions::whole`, off by default, routes every net against
every other at once on one grid before the passes, as PathFinder does and as the router
that `guide` is to replace in cadlab does (`pcb::whole`; the
[decision](architecture.md#decisions) has how). Measured on 2026-11-18 through cadlab's
adapter and its `route_bench`, on a machine that other work shared heavily: the counts
of a run with a deadline moved with the load from one hour to the next, so the two
routers were run side by side, and what decided between two versions of the code was a
budget of nodes, which is the same on any machine.

### What

The passes route a net and take others out for it. What 0.0.16 left open on
`corne-cherry`, 15 to 21 connections where cadlab's own router leaves 0 to 4, were the
pads of two packages of 0.4 mm pitch whose nets had been taken out and found no way back
past neighbours that had settled. In the negotiation every net is routed across every
other at a price, the wires in another's way are routed again, round after round at
prices that rise, and nothing is settled until none is in another's way: 432 of that
board's 432 connections in 42 to 70 rounds and some 60 million nodes, with none left
out when its routes are given to the world.

What it took beyond the rounds themselves:

- *The grid's origin to the nanometre.* Taken to the micrometre it was 5 nm beside the
  pads of the first package: a stub of 5 nm sideways and 0.4 mm long from each, and its
  neighbour's track 5 nm inside its clearance. 43 pieces of routes were left out by the
  world, and 39 connections with them.
- *Ways out for pads off the lines.* The second package is 15 µm off the lines that the
  first is on, and a track along the nearest line is 10 µm too close to the pad beside
  it: 30 terminals had no cell to end on. A pad with none gets ways out along its own
  axis that turn onto a line once clear of the row, three for each direction and line,
  and the negotiation chooses.
- *Only the wires that are in another's way are routed again*, and those that hung on
  them: routing every net near a shared point whole, as cadlab's router does, was 13
  rounds in the share where this is 60.
- *A connection with an end that is walled in is found by looking back from it*,
  20 000 nodes at most, before a search goes over the board for it: on
  `glasgow-revC3` 40 such connections were 16 of the 17 million nodes of the first
  round.
- *Pours.* With the routes of the first negotiation, cadlab's fill left 10 pads of the
  two grounds of `corne-cherry` cut off, with no room for a stub. The pads that the
  routes cut off their fills are now told when the rounds end, their nets planned again
  and the rounds go on. And the cells that tell it see a pad joined only by the spokes
  that the host gives, the spare ones too (`Zones::spare_spokes`).

### Measured

**A deadline of 60 s**, twelve boards of cadlab's corpus, four runs each: cadlab's own
router with seeds 1 to 4 on three threads, `guide` with `whole` in four orders of its
nets with three threads for the negotiation, two runs of each at a time, in the same
half hour. Open connections by run, as cadlab counts them on the board it fills, with
the least, the median and the most; the vias and length of the first run. This was the
source as committed first, before a negotiation near its end was given more of its
share.

| board | layers | cadlab's router | `guide` with `whole` |
|---|---|---|---|
| buspirate-flash-sop | 2 | 0, 0, 0, 0; 4 vias, 460 mm | 0, 0, 0, 0; 5 vias, 497 mm |
| buspirate-rs232 | 2 | 0, 0, 0, 0; 10, 281 mm | 0, 0, 0, 0; 4, 359 mm |
| lumenpnp-ringlight | 2 | 0, 0, 0, 0; 1, 177 mm | 0, 0, 0, 0; 4, 210 mm |
| nrfmicro | 2 | 0, 0, 0, 0; 46, 454 mm | 0, 0, 0, 0; 50, 554 mm |
| sweep-v2.2 | 2 | 3, 3, 3, 3; 21, 2161 mm | 4, 4, 3, 3 (3 / 3.5 / 4); 18, 2175 mm |
| corne-cherry | 2 | 3, 4, 0, 3 (0 / 3 / 4); 484, 8273 mm | 5, 8, 10, 5 (5 / 6.5 / 10); 617, 8738 mm |
| tinytapeout-demo | 4 | 16, 3, 15, 2 (2 / 9 / 16); 238, 4928 mm | 5, 6, 18, 18 (5 / 12 / 18); 455, 5557 mm |
| buspirate5-rev10 | 4 | 4, 2, 15, 7 (2 / 5.5 / 15); 314, 3045 mm | 0, 0, 0, 1 (0 / 0 / 1); 403, 3513 mm |
| lumenpnp-mobo | 4 | 64, 58, 23, 10 (10 / 40.5 / 64); 263, 5177 mm | 3, 1, 8, 4 (1 / 3.5 / 8); 531, 6322 mm |
| cynthion | 4 | 33, 36, 35, 38 (33 / 35.5 / 38); 474, 3152 mm | 12, 30, 11, 11 (11 / 11.5 / 30); 674, 4000 mm |
| glasgow-revC3 | 4 | 45, 44, 47, 48 (44 / 46 / 48); 418, 3538 mm | 35, 34, 34, 33 (33 / 34 / 35); 398, 3707 mm |
| glasgow-revD1 | 6 | 1136 in every run; 112 | 306, 315, 295, 267 (267 / 300.5 / 315); 948 |

The machine was the limit in that half hour: cadlab's router ended its minute on
`glasgow-revD1` with 112 vias placed, where it has 241 to 288 connections open on a quiet
machine, and `guide`'s own passes, which is all that board has (below), had 116 to 135
with 0.0.16. So the table says how the two stand under the same load, and no more.
`nrfmicro` is closed in every run, the first in 18 s, where 0.0.16 had 0 to 2 open at
the deadline. `corne-cherry` has 5 to 10 for 15 to 21, and is still behind 0 to 4.

**As released**, 0.0.17 from crates.io behind cadlab's adapter, which turns `whole` on
in the first run of a run with a budget, the same four runs of each side by side on a
quiet machine: the twelve boards 173 to 191 open (median 185.5) against 324 to 340
(335), the eleven without `glasgow-revD1` 55 to 59 against 81 to 101. `nrfmicro` 0, 0, 0
and 1 against 0 in every run, closed in 7 to 18 s in the three; `sweep-v2.2` 3 in every
run of both; `corne-cherry` 5, 8, 10 and 5 against 3, 4, 0 and 3, as in the table above
to the connection, so that it is not the load that leaves it behind; `tinytapeout-demo`
0 to 4 against 2 to 15, `buspirate5-rev10` 0 against 2 to 12, `lumenpnp-mobo` 0 against 4
to 13, `cynthion` 11 to 12 against 28 to 29, `glasgow-revC3` 31 to 35 against 31 to 36,
`glasgow-revD1` 118 to 134 against 239 to 250. Vias over the four runs: `lumenpnp-mobo`
409 to 529 against 336 to 343, `tinytapeout-demo` 379 to 494 against 240 to 290,
`sweep-v2.2` 18 to 24 against 21 to 28, `glasgow-revC3` 375 to 388 against 415 to 439.
Pads short of two spokes: 88 to 97 with 1 to 3 that have none, against 154 to 162 and
35 to 45. The curated boards, which are routed with no budget and with `whole` off:
1150 of 1151 connections with 712 vias, as before.

What `corne-cherry` has open in those runs are pads of its two grounds and of a supply
that cadlab's fill leaves cut off from their pours once the run is over, between other
pads with a track at each end, and no room for a stub: the run itself ends with 482 of
its 482 connections, by the cells that stand for the fill here. With a budget of nodes,
where cadlab runs no later run that moves routes, the board has 3 open (below). The
other router fills the zones itself between two negotiations; the pour rounds here need
the host's fill in place of the cells, as a call back into the host.

**The passes on six layers, timed call by call.** `glasgow-revD1` at 40 million nodes
with `whole` off, a timer round each call that a turn of the passes makes: 33.1 s in
the passes, 31.2 s of it inside the searches themselves (22.9 s in 3052 from the tree,
8.3 s in 3464 looks from the terminals), 0.8 s telling the nets' pieces, 0.8 s pruning.
The third of a pass that was put outside the searches after 0.0.16 is not between the
turns. A step of those searches is an exact query, 1.3 million nodes a second, where a
step of the negotiation is a look at its grid, 3 million.

**A budget of 150 million nodes**, two orders, which lets the negotiation end on the
boards it ends on: `corne-cherry` 3 and 3 open, `tinytapeout-demo` 0 and 0,
`buspirate5-rev10` 0 and 0, `lumenpnp-mobo` 0 and 0, `cynthion` 11 and 9, `glasgow-revC3`
28 and 27, `sweep-v2.2` 3 and 3. That is the result that a minute does not always have
the time for.

**A budget of 40 million nodes**, three orders, the nine boards that a budget cuts, as
released, beside 0.0.16 (in brackets): `nrfmicro` 0 (5), `sweep-v2.2` 9 (9),
`corne-cherry` 79 (75), `tinytapeout-demo` 43 (35), `buspirate5-rev10` 0 (16),
`lumenpnp-mobo` 0 (0), `cynthion` 38 (53), `glasgow-revC3` 105 (106), `glasgow-revD1` 441
(433): 715 for 732. The negotiation has half of the budget, which on `corne-cherry` and
`tinytapeout-demo` ends it a third of the way through: what it has then are routes with
shared points taken out, and the passes go on from those with the other half.

**Vias**, first runs of the table above against 0.0.16's of the day before: `sweep-v2.2`
18 for 42, `tinytapeout-demo` 455 for 598, `lumenpnp-mobo` 531 for 620, `glasgow-revC3`
398 for 559, `buspirate5-rev10` 403 for 408; `corne-cherry` 617 for 554 and `cynthion` 674
for 536, the boards on which it also joins the most more. At 150 million nodes
`lumenpnp-mobo` has 414 and 524 and `tinytapeout-demo` 395 and 412. Of
`lumenpnp-mobo`'s, 250 are stubs to its planes.

**Pads short of two spokes** (cadlab's count on its fill): 84 to 96 of 2360 in the four
runs, with 0 to 2 that have none, where 0.0.16 had 82 to 100 and 2; cadlab's router had
108 to 116 and 3 to 18 in the same half hour.

**A net that the negotiation ends with in pieces keeps them.** `sweep-v2.2` had 5, 4 and
4 open at 40 million nodes for 3, 3 and 3, and 4 in two runs of four at a minute. Three
of its nets have a connection that the grid has no way for. The passes on the finer
lattice looked for it, found none, and took the net out whole to route it again from
nothing, as they do with a net that could not be mended: on a board with every other net
routed round it by then, where a budget ended before it was whole again. A net whose
copper began as what a negotiation that ended left it with is no longer taken out so
(`corne-cherry`, whose negotiation 40 million nodes cut short, had 88 open over three
orders for 79 with the same for every net of a negotiation).

### Which boards

`whole` is for a run with a budget on a board that the negotiation has the time for,
and it says so itself: the first round takes the nets in strides through their order,
and when it has routed 15 % of their length, a round is that much over 0.15. A board
with no room for three rounds in the share is given up there. Of the twelve, that is
`glasgow-revD1`: 1939 connections on six layers at a pitch of 0.1 mm, some 10 million
nodes a round and twice that in the second, against 3 million on `corne-cherry`. Two
rounds were all that 21 s held, and the passes had 290 open after them for 180 without.
Given up after a second, the board is routed as 0.0.16 routes it.

### Tried and not kept

- *The halo of a stub made wider by what the grid's points miss between them*, as
  cadlab's router has it for copper off its grid: two ways out of pads 0.4 mm apart are
  then in each other's way for good, and `nrfmicro`'s negotiation stayed at 52 to 68
  shared points where it ends at 0 to 22 without. What a halo misses is found when the
  routes are given to the world.
- *Every net near a shared point routed again whole* (above).
- *A weight on the bound of the search*, 1.3 and 1.6: at 60 million nodes
  `corne-cherry` 25 and 27, and 23 and 24, for 17 and 20; `tinytapeout-demo` 11 and 11,
  and 8 and 5, for 4 and 4.
- *The spokes of a pad as something that only so many nets may cross*, counted by pad
  while the nets are routed, with a price and a history as a shared point has: at 150
  million nodes `tinytapeout-demo` 7 and 1 open for 0 and 0, `corne-cherry` 5 and 3 for
  3 and 3, four boards the same to the via, and 51 and 53 pads short of two spokes for
  50.
- *Regions round the wires' own boxes, and a first round in fixed batches*, for more
  nets side by side: 100 rounds without an end on `corne-cherry` for 70, and 10.3 s on
  three threads for 12.7 s.
- *Whether the negotiation is worth beginning, told by twelve times the first 15 % of a
  round in the nets' own order*, shortest first: `corne-cherry` 9 s said for 19 s
  taken, `cynthion` 109 s said for 19 s taken.

### Threads

Nets whose regions are apart are routed side by side. On `corne-cherry` three threads
end the negotiation in 12.7 s for 15.0 s on one, with the same routes: its nets span the
board, 1.8 to a batch on average. The counts do not depend on it.

### Open

- `corne-cherry` at a minute on a loaded machine, 5 to 10 for 0 to 4. The negotiation
  ends in 15 s on a quiet one, and what it has when it is cut short is not as good as
  what the other router has at the same moment, which makes its rounds in a third of
  the nodes.
- A board of six layers at a pitch of 0.1 mm: a round is 10 million nodes and more.
- Vias: `tinytapeout-demo` 395 to 455 and `lumenpnp-mobo` 414 to 531, where cadlab's
  router has 238 and 263 to 340.

## PCB routing on the host's own fills

What was changed after [the release above](#pcb-routing-of-every-net-at-once), 0.0.17,
for the same host, cadlab: `RunControl::refill`, a call back into a host that fills its
zones itself, for its fill around the routes as they stand
([decision](architecture.md#decisions)). Measured on 2026-11-18 through cadlab's adapter
and its `route_bench`, on a machine that other work shared: what decided between two
versions of the code was a budget of nodes, which is the same on any machine, and the
runs with a deadline are those of the adapter as it was when each was made.

### What

`corne-cherry` was the one board of cadlab's twelve where its own router left fewer
connections open than `guide`: 0 to 4 against 5 to 10 at 60 s. Each of those was a pad of
a ground or a supply that `guide` had told joined, 482 of 482 connections on its own
board, and that cadlab's fill around the routes left off its pour. The cells that
`guide` tells the pads cut off by (`pcb::pours`) keep a place between two tracks as
filled where the host's fill, with its least width and its islands taken out, has
nothing, and a stub that ended there joined its pad to no copper. Cadlab's router fills
the zones itself between two negotiations.

- **The host's fill.** `RunControl::refill` is given the tracks and vias of the run and
  returns the host's fills. The pieces are then those, to the nanometre: a pad is on the
  pieces that touch it, a route on those that hold its ends and its vias. On a board
  with a fill planned on an outer layer it is asked for when the negotiation for every
  net tells the pads cut off, three times at most, and at each round of stubs once
  routing is over, three at most; on any board once more when cleanup is over.
- **A wire to a place that is no longer fill.** The negotiation kept a wire that ended
  at a place in a fill as joined to the fill when that place was on no piece of it any
  more, and never routed its pad again. This is what the host's fills showed, and it
  was so with the cells too. Such a wire now goes.
- **More rounds for the pads cut off, and another order.** Each time the pads cut off
  are told, the negotiation has forty more rounds: `corne-cherry`'s ended at its
  hundredth with a second telling still to come. One that ends on its own with points
  still shared, having cost no more than half of what its share has left, is begun again
  with the nets moved another way, three times at most, and the one with the fewest
  shared is kept.
- **Stubs that a pour makes needless.** A net with a plane has a stub for every pad off
  the plane. By the fill asked for when cleanup is over, a stub whose ends the host's
  fills, the board's copper and the net's other routes join without it is taken out,
  and returned apart (`Routed::spare`) for a host that runs again from the routes.

### Results

At a budget of nodes, which gives the same routes from source and from the crate, the
nine boards that a budget cuts, three orders of the nets, connections open, 0.0.17
first:

| board | 40 million nodes, 0.0.17 | with this | 150 million, two orders, 0.0.17 | with this |
|---|---|---|---|---|
| nrfmicro | 0 | 0 | | 0, 0 |
| sweep-v2.2 | 9 | 9 | 3, 3 | 3, 3 |
| corne-cherry | 79 | 79 | 3, 3 | 2, 1 |
| tinytapeout-demo | 43 | 41 | 0, 0 | 0, 2 |
| buspirate5-rev10 | 0 | 0 | 0, 0 | 0, 0 |
| lumenpnp-mobo | 0 | 0 | 0, 0 | 0, 0 |
| cynthion | 38 | 35 | 11, 9 | 11, 11 |
| glasgow-revC3 | 105 | 105 | 28, 27 | 30, 28 |
| glasgow-revD1 | 441 | 441 | | |
| all | 715 | 710 | | |

At 40 million nodes the negotiation is cut a third of the way through on
`corne-cherry`, before any pad cut off is told. At 150 million, where it ends, that
board has 2 and 1 open for 3 and 3. The three boards that have one or two more at 150
million are boards with planes on inner layers, where no fill is asked for while
routing: their first runs are the same as with 0.0.17 to the node, and what differs is
which of cadlab's later runs, each in another order, ends with less open.
`tinytapeout-demo`'s second order has 4 open after its first run and its run on the
fixed board with either version; with 0.0.17 the runs that had the routes back closed
them, and here they closed two.

At 60 s, through the adapter, four orders of the nets, three threads for the
negotiation, with other work on the machine:

- `corne-cherry`: 2, 1, 0 and 0 open, for 5 to 10 with 0.0.17 and 0 to 4 for cadlab's
  router. With the host's fills and the wires to places that are fill no more still
  kept: 4, 6, 6 and 2. The fill of that board takes cadlab 0.35 s, and its first run
  asks for it five to seven times.
- `nrfmicro`: 0 in all four, for 0, 0, 0 and 1. Its negotiation never ends with nothing
  shared: 20 to 64 points in each of sixteen attempts, the best of each four 20, 23, 15
  and 10. The order that had left a connection of its USB receptacle open closes in 3 s.
- Vias, by the stubs taken out: `lumenpnp-mobo`'s first run ends with 133 stubs to its
  planes for 254, and the board with 402 to 408 vias in three orders at 40 million
  nodes for 416, 523 and 529, the two higher counts being runs in which cadlab's own
  pass for the same, which is two fills once all its runs are over, was not made;
  `tinytapeout-demo` 96 stubs for 122.

### Tried and not kept

- **The host's fill at every telling on every board.** On `lumenpnp-mobo`, whose nets
  with zones are planned on inner planes, which few routes cross, four fills of 1.4 s
  each of the first run's 42 s, and none left for the stubs at the end. Hence the
  fills while routing only where a fill is planned on an outer layer.
- **The stubs taken out and not given back.** The first version left them out of the
  routes for good. A later run of the host that takes the routes back plans a net with
  a plane on the plane alone, makes each stub again where there is room, and ended with
  72 connections open on its own board for 7: `tinytapeout-demo` had 4 open at 150
  million nodes for 0. Hence `Routed::spare`.
- **Dearer vias in the negotiation**, for the vias that the host counts beside its own
  router's (`lumenpnp-mobo` 409 to 529 against 336 to 343 with 0.0.17). A via at 24 and
  at 40 steps of the grid for 10, at 40 million nodes over three orders on the nine
  boards: 765 and 794 connections open for 710, with 11355 and 11085 vias for 11851
  (`corne-cherry` 102 and 85 open for 79, `cynthion` 47 and 63 for 35,
  `tinytapeout-demo` 48 and 65 for 41, and at 24 with more vias, not fewer). At 60 s, where
  more negotiations end, two orders at 24 had as few open on five boards of six and
  `corne-cherry` 433 and 435 vias for 535 and 626: it is a gain where the rounds end
  and a loss where the budget ends them, and the default stays.

### As released

0.0.18 from crates.io behind cadlab's adapter (cadlab's D114), beside cadlab's own
router, on 2026-11-18 with nothing else on the machine: 60 s a board, four runs of
cadlab's router with seeds 1 to 4 on three threads and four orders of `guide`'s nets
with three threads for its negotiation, two runs of each at a time, in the same half
hour. Connections open:

| board | layers | cadlab's router: open by run (min / median / max); vias, length, time | guide 0.0.18: open by run (min / median / max); vias, length, time |
|---|---|---|---|
| buspirate-flash-sop | 2 | 0, 0, 0, 0 (0 / 0 / 0); 4, 460.2 mm, 0.5 s | 0, 0, 0, 0 (0 / 0 / 0); 5, 497.1 mm, 0.4 s |
| buspirate-rs232 | 2 | 0, 0, 0, 0 (0 / 0 / 0); 10, 280.9 mm, 0.5 s | 0, 0, 0, 0 (0 / 0 / 0); 4, 358.6 mm, 0.4 s |
| lumenpnp-ringlight | 2 | 0, 0, 0, 0 (0 / 0 / 0); 1, 176.5 mm, 0.4 s | 0, 0, 0, 0 (0 / 0 / 0); 4, 206.1 mm, 0.7 s |
| nrfmicro | 2 | 0, 0, 0, 0 (0 / 0 / 0); 46, 453.6 mm, 11.4 s | 0, 0, 0, 0 (0 / 0 / 0); 48, 536.2 mm, 19.2 s |
| sweep-v2.2 | 2 | 3, 3, 3, 3 (3 / 3 / 3); 21, 2160.9 mm, 51.8 s | 3, 3, 3, 3 (3 / 3 / 3); 18, 2210.5 mm, 58.4 s |
| corne-cherry | 2 | 3, 4, 0, 3 (0 / 3 / 4); 484, 8273.0 mm, 28.9 s | 2, 1, 0, 0 (0 / 0.5 / 2); 625, 8906.3 mm, 58.4 s |
| tinytapeout-demo | 4 | 15, 3, 15, 2 (2 / 9 / 15); 238, 4930.2 mm, 60.2 s | 0, 4, 0, 4 (0 / 2 / 4); 427, 5610.8 mm, 58.3 s cut |
| buspirate5-rev10 | 4 | 4, 2, 12, 5 (2 / 4.5 / 12); 314, 3044.6 mm, 30.4 s | 0, 0, 0, 0 (0 / 0 / 0); 368, 3469.5 mm, 46.4 s cut |
| lumenpnp-mobo | 4 | 13, 9, 4, 33 (4 / 11 / 33); 340, 5370.3 mm, 62.0 s cut | 0, 0, 0, 0 (0 / 0 / 0); 408, 6292.1 mm, 53.2 s |
| cynthion | 4 | 28, 29, 29, 28 (28 / 28.5 / 29); 476, 3176.6 mm, 62.2 s cut | 12, 11, 12, 11 (11 / 11.5 / 12); 643, 3959.9 mm, 55.5 s cut |
| glasgow-revC3 | 4 | 36, 37, 35, 31 (31 / 35.5 / 37); 415, 3436.4 mm, 60.2 s cut | 35, 33, 31, 34 (31 / 33.5 / 35); 392, 3650.4 mm, 58.9 s cut |
| glasgow-revD1 | 6 | 241, 243, 238, 250 (238 / 242 / 250); 898, 7744.3 mm, 63.6 s cut | 122, 135, 118, 134 (118 / 128 / 135); 1253, 8718.3 mm, 57.6 s cut |
| all twelve | | 343, 330, 336, 355 (330 / 339.5 / 355) | 174, 187, 164, 186 (164 / 180 / 187) |
| the eleven without glasgow-revD1 | | 102, 87, 98, 105 (87 / 100 / 105) | 52, 52, 46, 52 (46 / 52 / 52) |

No DRC error but connections open in the 96 runs, and no call of `guide` over 59.3 s.
`guide` is behind in every run on no board: `corne-cherry` has 0 to 2 open against 0 to
4, for 5 to 10 with 0.0.17, and `nrfmicro` none in any order, for one in one of four.
Against 0.0.17 a day earlier nothing has more open by its range (`glasgow-revD1` 118 to
135 for 118 to 134), and the twelve boards have 164 to 187 for 173 to 191.

What it does not reach: within 15 % of cadlab's router's vias on `lumenpnp-mobo` (402 to
411 against 309 to 343, for 409 to 529) and on `tinytapeout-demo` (385 to 427 against
238 to 290, for 379 to 494). The stubs went; the vias of the other nets are as they
were. The pads in pours short of two spokes, cadlab's count over the twelve boards, are
90 to 100 in the four orders for 88 to 97 with 0.0.17, against 154 to 162 after
cadlab's router. At 40 million nodes the crate gives the 710 that the source gave.

In cadlab's adapter, not here, and found by this: a fill that takes long (5.7 s on
`glasgow-revD1`) asked for after the deadline cost that board its second run, 136 to
150 open for 118 to 135, so the adapter refuses the last fill after a deadline unless
a fill was timed and took no more than 3 % of the budget.

### What this does not show

- A host other than cadlab. The pieces are told with `planes::pieces`, which takes a
  fill for a polygon with holes that touches the pads it joins; a host whose fill
  leaves a gap at a pad and joins it by tracks of its own would have every pad cut off.
- Boards whose fill takes long. Cadlab's adapter gives a fill while routing only within
  15 % of the run's time, and the one at the end after the deadline only if a fill is
  within 3 % of the whole budget: on `glasgow-revD1`, some 5 s a fill, the stubs are
  taken out when the first run ends before its deadline and not otherwise.
- The vias of the other nets. Of `lumenpnp-mobo`'s some 280 are not stubs, where
  cadlab's router has some 207, and nothing here touches them.

## PCB routing at twice the pitch

What was changed after [the release above](#pcb-routing-on-the-hosts-own-fills), 0.0.18,
for the same host, cadlab: where its own router still had the better copper, vias and
track, and the boards on which both leave the most open
([decisions](architecture.md#decisions)). Measured on 2026-11-18 through cadlab's adapter
and its `route_bench`. What decided between two versions of the code was a budget of
nodes, which is the same on any machine; the runs with a deadline are those of the
source as it was when each was made, several at a time.

### What

At 60 s 0.0.18 was behind cadlab's router on no board by the connections it left open,
and behind it by its vias: `lumenpnp-mobo` 402 to 411 against 309 to 343,
`tinytapeout-demo` 385 to 427 against 238 to 290, `corne-cherry` 534 to 625 against 445
to 505, `glasgow-revD1` 1202 to 1253 against 877 to 932.

**Where the vias come from**, counted in one run of each board:

- *The negotiation's vias only rise.* After its first round, each net where it would go
  alone, `tinytapeout-demo` has 310 vias, and 485 when its share ends it at the
  forty-second; `lumenpnp-mobo` 442 and 534 at the twelfth, where it ends with nothing
  shared; `corne-cherry` 319 and 666 at the hundred and eighteenth. Only the wires in
  another net's way are routed again, and a wire that went under another stays there
  when the other has moved. The pass for fewer vias at the end, each net once more
  with nothing shared and vias at four times their price, takes 32, 43 and 51 of them
  on three boards, and none where the share ended the rounds first.
- *The passes add more.* What the negotiation leaves open the passes route, a net at a
  time with others taken out for it, at a quarter of the negotiation's price for a
  via: `tinytapeout-demo`, with all of what follows, begins them with 219 to 278 vias
  in the nets that have no plane and ends them with 313 to 379, for 15 to 22
  connections more.
- *A net with a pour has more than one via for each piece of it.* By the nets with
  zones and the others, one run: `lumenpnp-mobo` 163 and 245 against 140 and 200 for
  cadlab's router, `tinytapeout-demo` 111 and 345 against 43 and 195, `corne-cherry` 171
  and 454 against 75 and 409. The routes of both routers leave the pours in as many
  pieces: on `corne-cherry` the pads and fills of one ground are in 89 pieces without
  the net's own routes after `guide` and in 79 after cadlab's router, which joins them
  with 37 vias where `guide` has 83. A net whose pads are cut off its pour is joined up
  again at each of the three times that is told, and what one telling joined through a
  via the next finds joined by the pour.

**What was done:**

- **On more than two layers the grid is at twice the pitch.** A track and its
  clearance apart, where it was half that: two tracks side by side are still a step
  from each other, there are a quarter of the points, a round costs a fifth of the
  nodes, and a via costs twice the track it did. A board of two layers stays at the
  first pitch, and has the second only where the first round says that its share has
  no room for three rounds of the first.
- **A negotiation at twice the pitch is begun if one first round fits the time**, or
  thirty first rounds a budget of nodes: `glasgow-revD1`, six layers and 1939
  connections, was given up after a second at the first pitch. At twice it has five
  rounds in its share, which leave 3700 points shared and 130 nets with wires taken
  out, and the passes begin from the rest.
- **There, a negotiation that its share ends has its end all the same**: the nets
  taken out routed again where nothing is claimed, and the pass for fewer vias, the
  nets with the most first, within 2.5 times what the first round took and 15 % of
  what the run has left.
- **Every via is tried against the fills**, where it was the stubs: a via goes if the
  pads of its net are in the same pieces without it, told by the copper itself, and
  then the tracks that this leaves leading nowhere. A small piece of fill with no pad
  on it is joined to nothing for its own sake. `needless` gives the same to a host
  that fills its board after a run, with nothing filled for it.
- **A via may be in a pad that the host names and that no track leaves**
  (`Zones::via_pads`): the exposed pad under a package, which the pads round it shut
  in. `cynthion` has three, joined by vias in them on its own board and by no router.

### Results

At a budget of 40 million nodes, the nine boards that a budget cuts, three orders of the
nets: connections open over the three, and the range of the vias, 0.0.18 first.

| board | layers | open, 0.0.18 | with this | vias, 0.0.18 | with this |
|---|---|---|---|---|---|
| nrfmicro | 2 | 0 | 0 | 48 to 60 | 48 to 60 |
| sweep-v2.2 | 2 | 9 | 9 | 18 to 24 | 18 to 24 |
| corne-cherry | 2 | 79 | 79 | 494 to 519 | 473 to 501 |
| tinytapeout-demo | 4 | 41 | 30 | 397 to 454 | 362 to 412 |
| buspirate5-rev10 | 4 | 0 | 0 | 377 to 393 | 314 to 323 |
| lumenpnp-mobo | 4 | 0 | 0 | 401 to 408 | 331 to 335 |
| cynthion | 4 | 35 | 24 | 603 to 669 | 405 to 421 |
| glasgow-revC3 | 4 | 105 | 100 | 395 to 400 | 341 to 384 |
| glasgow-revD1 | 6 | 441 | 433 | 1078 to 1134 | 1066 to 1107 |
| all | | 710 | 675 | | |

The boards of two layers are as they were but for the vias that the fills make needless.
`glasgow-revD1` has no negotiation at this budget, at either pitch: thirty first rounds
are 61 million nodes. `cynthion`'s eleven fewer are nine for the three exposed pads and
two for the grid.

At 60 s, through the adapter, four orders of the nets with three threads for the
negotiation, four runs at a time on the machine, the source as it was before the small
pieces of fill with no pad stopped counting; 0.0.18 in brackets, from the table of the
[release above](#pcb-routing-on-the-hosts-own-fills), two runs at a time:

| board | open by order | vias |
|---|---|---|
| nrfmicro | 0, 0, 0, 0 (0, 0, 0, 0) | 41 to 60 |
| sweep-v2.2 | 3, 3, 3, 3 (3, 3, 3, 3) | 18 to 24 |
| corne-cherry | 2, 1, 0, 0 (2, 1, 0, 0) | 529 to 619 (534 to 625) |
| tinytapeout-demo | 1, 4, 1, 0 (0, 4, 0, 4) | 333 to 465 (385 to 427) |
| buspirate5-rev10 | 0, 0, 0, 0 (0, 0, 0, 0) | 311 to 351 (349 to 372) |
| lumenpnp-mobo | 0, 0, 0, 0 (0, 0, 0, 0) | 337 to 354 (402 to 411) |
| cynthion | 8, 8, 8, 8 (12, 11, 12, 11) | 427 to 470 (604 to 699) |
| glasgow-revC3 | 29, 30, 33, 34 (35, 33, 31, 34) | 325 to 359 (379 to 403) |
| glasgow-revD1 | 103, 87, 101, 99 (122, 135, 118, 134) | 1092 to 1125 (1202 to 1253) |

Pads in pours short of two spokes, cadlab's count over the twelve boards: 70 to 80 in
the four orders for 90 to 100, with 0 to 3 that have none for 1 to 3;
`buspirate5-rev10` 29 to 34 for 33 to 43, `glasgow-revD1` 24 to 28 for 38 to 42,
`tinytapeout-demo` 11 to 13 for 6 to 10.

`glasgow-revD1` alone, two orders, two runs at a time: 125 and 135 open with no
negotiation, 82 and 84 with one at twice the pitch in half of the first run, 79 and 74
with three quarters of it.

### Tried and not kept

- **The history of the rounds forgotten before the pass for fewer vias, and the pass
  made up to four times.** A search with nothing shared still pays for a point what the
  rounds added to it each time it was shared. Without that and with more rounds, at 150
  million nodes: `lumenpnp-mobo` 31 vias saved in the first and none in the second,
  `buspirate5-rev10` 52 and 3, `glasgow-revC3` 28 and 1, and no fewer on the routed
  board, 411 for 408 and 357 for 346. With the others where they are, a net has no
  better way.
- **The nets with something shared routed again whole**, every round, or every fourth:
  at 150 million nodes the nets with no plane have 247 and 243 vias on `lumenpnp-mobo`
  for 245, 245 and 249 on `buspirate5-rev10` for 247, and on `corne-cherry` and
  `tinytapeout-demo` the budget ended the rounds with more open.
- **Every net near a shared point routed again whole**, as cadlab's router does it, in
  every round or in the first eight, at 300 million nodes, which lets every negotiation
  end: `corne-cherry` 518 vias for 625 at 2.6 times the nodes, `buspirate5-rev10` 392
  for 346, `tinytapeout-demo` 407 for 389; in the first eight rounds only, 607, 357 and
  383. Nothing that a minute pays for.
- **Twice the pitch on two layers.** `nrfmicro` at 60 s in four orders: 0, 2, 2 and 1
  open for none, and 4 for none over three orders at 40 million nodes; `sweep-v2.2`
  the same either way.
- **The first pitch with three quarters of the run on six layers**: `glasgow-revD1` 161
  and 198 open for 125 and 135 with no negotiation; at twice the pitch 82 and 84 with
  half, 79 and 74 with three quarters.
- **A negotiation at twice the pitch begun whenever one first round fits its budget of
  nodes**: `glasgow-revD1` at 40 million has two rounds in its 20, and 599 open over
  three orders for 441.
- **The end of a negotiation that was cut, at the first pitch too**: `corne-cherry` at
  40 million nodes 95 open over three orders for 79. At twice the pitch
  `tinytapeout-demo` has 30 with it for 26 without at that budget, and at 60 s 0, 3, 1
  and 4 with it for 3, 7, 0 and 7.
- **Tracks from pad to pad taken out where a fill joins the two.** The room goes to
  another net's pour, where the board has one with a higher priority on the layer, and
  the fill that joined the pads is then another: on `cynthion` a strip of the ground's
  pour beside such a track was gone with it, and the pad at its end open. Four runs of
  27 ended with a connection more for it, which the host's check found.
- **Vias let into every pad that the host names**, shut in or not: `corne-cherry`'s two
  packages have a ground pin beside their exposed pads, and with the room under them
  taken by vias the board had 1, 4 and 3 open at 150 million nodes for 2, 1 and 0.
- **The rooms of spokes told half a step round each point of the coarser grid**, for
  the ones that lie between two points: `buspirate5-rev10` 33 to 36 pads short of two
  spokes at 40 million nodes for 31, with a connection open in one order of three.
- **Dearer vias in the passes after a negotiation**, 2 mm of track for 1: at 40 million
  nodes over three orders `corne-cherry` 88 open for 79 and `glasgow-revC3` 103 for
  100, with as many vias on `tinytapeout-demo`, 362 to 422 for 370 to 418.

### As released

0.0.19 from crates.io behind cadlab's adapter (cadlab's D115), which names the heat pads
in `Zones::via_pads` and asks `needless` with the fill that counts its last run, beside
cadlab's own router, on 2026-11-18 with nothing else on the machine: 60 s a board, four
runs of cadlab's router with seeds 1 to 4 on three threads and four orders of `guide`'s
nets with three threads for its negotiation, two runs of each at a time, in the same
half hour. Connections open:

| board | layers | cadlab's router: open by run (min / median / max); vias, length, time | guide 0.0.19: open by run (min / median / max); vias, length, time |
|---|---|---|---|
| buspirate-flash-sop | 2 | 0, 0, 0, 0 (0 / 0 / 0); 4, 460.2 mm, 0.5 s | 0, 0, 0, 0 (0 / 0 / 0); 5, 497.1 mm, 0.4 s |
| buspirate-rs232 | 2 | 0, 0, 0, 0 (0 / 0 / 0); 10, 280.9 mm, 0.5 s | 0, 0, 0, 0 (0 / 0 / 0); 4, 358.6 mm, 0.4 s |
| lumenpnp-ringlight | 2 | 0, 0, 0, 0 (0 / 0 / 0); 1, 176.5 mm, 0.4 s | 0, 0, 0, 0 (0 / 0 / 0); 4, 205.3 mm, 0.8 s |
| nrfmicro | 2 | 0, 0, 0, 0 (0 / 0 / 0); 46, 453.6 mm, 11.3 s | 0, 0, 0, 0 (0 / 0 / 0); 48, 536.2 mm, 19.1 s |
| sweep-v2.2 | 2 | 3, 3, 3, 3 (3 / 3 / 3); 21, 2160.9 mm, 51.3 s | 3, 3, 3, 3 (3 / 3 / 3); 18, 2210.5 mm, 58.4 s |
| corne-cherry | 2 | 3, 4, 0, 3 (0 / 3 / 4); 484, 8273.0 mm, 28.9 s | 2, 1, 0, 0 (0 / 0.5 / 2); 604, 8804.6 mm, 58.3 s |
| tinytapeout-demo | 4 | 14, 3, 15, 2 (2 / 8.5 / 15); 240, 4932.7 mm, 61.4 s | 0, 7, 1, 0 (0 / 0.5 / 7); 372, 6066.9 mm, 59.1 s cut |
| buspirate5-rev10 | 4 | 4, 2, 12, 5 (2 / 4.5 / 12); 314, 3044.6 mm, 30.1 s | 0, 0, 0, 0 (0 / 0 / 0); 308, 3569.4 mm, 22.3 s |
| lumenpnp-mobo | 4 | 13, 9, 5, 33 (5 / 11 / 33); 340, 5370.3 mm, 61.8 s cut | 0, 0, 0, 0 (0 / 0 / 0); 331, 6686.0 mm, 54.4 s |
| cynthion | 4 | 28, 29, 29, 28 (28 / 28.5 / 29); 476, 3176.6 mm, 62.0 s cut | 8, 8, 8, 8 (8 / 8 / 8); 405, 3762.2 mm, 57.9 s |
| glasgow-revC3 | 4 | 36, 31, 35, 31 (31 / 33 / 36); 415, 3436.4 mm, 61.4 s cut | 29, 30, 33, 34 (29 / 31.5 / 34); 359, 4006.2 mm, 59.4 s cut |
| glasgow-revD1 | 6 | 241, 288, 376, 307 (241 / 297.5 / 376); 898, 7744.3 mm, 64.2 s cut | 105, 93, 112, 105 (93 / 105 / 112); 1112, 9233.4 mm, 58.0 s cut |
| all twelve | | 342, 369, 475, 412 (342 / 390.5 / 475) | 147, 142, 157, 150 (142 / 148.5 / 157) |
| the eleven without glasgow-revD1 | | 101, 81, 99, 105 (81 / 100 / 105) | 42, 49, 45, 45 (42 / 45 / 49) |

No DRC error but connections open in the 96 runs, and no call of `guide` over 59.8 s.
`guide` is behind in every run on no board, and ahead in every run on four. Against
0.0.18 a day earlier: `cynthion` 8 open in every order for 11 to 12, `glasgow-revD1` 93
to 112 for 118 to 135, `glasgow-revC3` 29 to 34 for 31 to 35, `corne-cherry` 2, 1, 0 and
0 as before, and `tinytapeout-demo` 0, 7, 1 and 0 for 0, 4, 0 and 4: one order has more,
whose first run left 4 and whose run on the fixed board ended with no time for one that
moves routes. The twelve boards have 142 to 157 for 164 to 187.

Vias, the range of the four runs against cadlab's router's, and 0.0.18's in brackets:
`lumenpnp-mobo` 331 to 348 against 309 to 343 (402 to 411), `buspirate5-rev10` 308 to
348 against 291 to 314 (349 to 372), `cynthion` 405 to 430 against 469 to 482 (604 to
699), `glasgow-revC3` 330 to 359 against 415 to 439 (379 to 403), `glasgow-revD1` 1080
to 1114 against 826 to 898 (1202 to 1253), `corne-cherry` 522 to 609 against 445 to 505
(534 to 625), `tinytapeout-demo` 362 to 462 against 240 to 290 (385 to 427). What it
does not reach is within 15 % of cadlab's router's median on two of the three boards it
was asked for: `lumenpnp-mobo` is there, 336 against 339.5; `tinytapeout-demo` is 41 %
over, for 54 %, with 296 to 370 vias in its nets without zones for 195 to 232; and
`corne-cherry` 22 % over, as it was, its two grounds with 134 to 162 vias for 63 to 75.

Track is longer than it was on the boards negotiated at twice the pitch: by the medians
of the four runs against cadlab's router's, and by the first runs of 0.0.18's table in
brackets, `tinytapeout-demo` 23 % over (14 %), `lumenpnp-mobo` 20 % (17 %),
`buspirate5-rev10` 17 % (14 %), `glasgow-revC3` 12 % (6 %), `glasgow-revD1` 21 %
(13 %); `cynthion` has less, 19 % (25 %).

Pads in pours short of two spokes, cadlab's count over the twelve boards: 73 to 77 in
the four orders for 90 to 100 with 0.0.18, against 154 to 170 after cadlab's router;
with none, 0 to 3 for 1 to 3. `buspirate5-rev10` 29 to 34 for 33 to 43,
`glasgow-revD1` 23 to 28 for 38 to 42, `tinytapeout-demo` 9 to 14 for 6 to 10.

At 40 million nodes the crate gives the 675 that the source gave, to the via. At 150
million, two orders, 0.0.18 in brackets: `corne-cherry` 2 and 1 (2 and 1),
`tinytapeout-demo` 0 and 3 (0 and 2), `cynthion` 8 and 8 (11 and 11), `glasgow-revC3`
24 and 26 (30 and 28), `buspirate5-rev10`, `lumenpnp-mobo` and `nrfmicro` none,
`sweep-v2.2` 3 and 3. The curated boards, with `whole` off and no host: 1150 of 1151
connections with 712 vias and 37556357 nodes, as before.

In cadlab's adapter, not here, and found by this: asked for only when there was time
for two fills of the board, `needless` was not asked in two of `tinytapeout-demo`'s
four orders, whose last runs came back at 56 and 57 s, and the board kept 430 and 490
vias there; with the time of one, and the second fill only where the first opens a
connection, it has 362 and 462. And the fill that a run asks for when its cleanup is
over is given only before the run's deadline now: what it told, `needless` tells by a
fill that the host makes anyway.

### What this does not show

- The vias that the passes add after a negotiation that was cut. `tinytapeout-demo`
  is where they are most: its negotiation does not end in a minute at either pitch,
  with 38 to 98 points shared when its share is over.
- A board of two layers with no room at the first pitch. The corpus has none, and the
  second pitch there is as it is on six layers, untried.
- A host whose pours have no priorities among them, or a board with one net's pour to
  a layer: a track from pad to pad that a fill makes needless could go there.

## PCB routing out of an array, and shorter after it

What was changed after [the release above](#pcb-routing-at-twice-the-pitch), 0.0.19, for
the same host, cadlab: the board on which both routers left the most open for its size,
whose connections all end at one array of balls, and the track, which was longer than
cadlab's router's on every board ([decisions](architecture.md#decisions)). Measured on
2026-11-18 through cadlab's adapter and its `route_bench`. What decided between two
versions of the code was a budget of nodes where that could, and four orders at a minute
where it could not; the runs with a deadline are those of the source as it was when each
was made, several at a time.

### What

**Where the connections of `glasgow-revC3` were left.** 29 to 34 at a minute, and every
one at a ball of its array of 121 at 0.8 mm, between two of which one track passes. Its
own board takes the first ring out on the top, the second between the balls of the
first, and every ball further in through a via of 0.4 mm on the diagonal away from the
middle, 46 of them, each quarter of the array to its own corner. `guide` gave the balls
further in their dog-bones towards the middle, and no ball of the second ring anything:
a net that came first went along the first ring, the shortest way from one side of the
array to the other, and closed the two ways out of every ball of the second ring that
it passed.

**Where the track was longer**, net by net beside cadlab's router's on
`tinytapeout-demo` and `lumenpnp-mobo` (`CADLAB_BENCH_BY_NET` writes a result's nets and
tracks): of the nets with no zone that both routers closed, 70 % and 64 % were more than
a tenth longer with `guide`, and 7 % and 3 % a twentieth shorter. Three things, by what
each was worth:

- *A net keeps the way round another that has moved since.* The routes of a negotiation
  are where the prices of its rounds put them, and a net that the passes took round
  another is not looked at again when that one goes. Cleanup pulled each run tight and
  looked for fewer vias; it looked for no other way.
- *A net with a pour is joined pad to pad by tracks that the pour joins as well.* It is
  joined up again each time the routes of others cut its pour, while those still move,
  and a pad off its plane goes to the pad beside it where that is nearer than a via:
  `lumenpnp-mobo`'s ground had 402 tracks and 448 mm for 20 vias, where cadlab's router,
  which plans on the pours, has 93 tracks and 124 mm for 21.
- *A track goes to the middle of its pad*, and cadlab's router's ends at the first point
  of its grid on the pad's copper. On `buspirate-rs232` `guide` had 359 mm for 281 mm,
  28 % more, of which 292 mm for 272 mm, 7 % more, is outside the pads of its net
  (`route_bench` tells that length too). That part is copper on copper, and is left as
  it is.

**What was done:**

- **The ways out of the second ring of an array are planned before anything else**, in
  a run with a budget: each ball of that ring, where one track passes between two
  balls, has a track to the middle of the four balls beside it and from there straight
  out past the first ring, as copper of its net, if that is free; for a net with no
  zone and no more than eight pads. The first ring gets out any way.
- **The dog-bones of the balls further in are on the side away from the middle**, where
  they were towards it.
- **`RouteOptions::shorten`**, off by default: in cleanup, each run of a net's tracks
  from one pad or branch to the next that costs more than the straight way between its
  ends, by three pitches and six hundredths of itself, is taken out, the longest way
  round first, and searched for again between the two pieces that this leaves the net
  in, within twelve pitches of where it was, with every other route where it is. The
  new one is kept if it costs less, a via at three times what it costs while routing.
  Three times through the nets, the second and third for the runs near one that moved;
  then every net is pulled tight again. The routes that a run was given and has not
  moved are looked at too, and what a run that went another way cuts off a planned fill
  is joined to it again afterwards.
- **`needless_between`**: as `needless`, and the tracks that the pads of their nets are
  joined without as well, the longest first, whatever they end at. For a host that
  fills its board again without them and looks at what that fill joins and what it
  leaves of its reliefs: a track from pad to pad stays in `needless`, as the fill
  without it is another.
- **At the first pitch the price of a point that is claimed stops at thirty times a
  step**, where it went to a thousand: what is still shared is the history's to price.
  At twice the pitch it is as it was.

### Results

`glasgow-revC3`, connections open in four orders at a minute, with each part:

| | open by order | sum |
|---|---|---|
| 0.0.19: dog-bones towards the middle, nothing for the second ring | 29, 30, 32, 34 | 125 |
| dog-bones away from the middle | 24, 27, 29, 25 | 105 |
| and the ways out of the second ring, for every net | 11, 9, 8, 11 | 39 |
| and those only for a net with no zone and no more than eight pads | 5, 4, 6, 7 | 22 |

At a budget of 40 million nodes, three orders, it has 41 open for 100. The other two
boards with such arrays: `glasgow-revD1`, 256 balls, 395 open over four orders at a
minute for 392 and 437 at 40 million nodes for 433; `cynthion` 8 in every order, as
before. With the ways out for every net `glasgow-revD1` had 426 for 388: a ball of a
supply in the second ring goes to its plane by a via where it is.

The nine boards that a budget of 40 million nodes cuts, three orders: 620 connections
open for 675 with 0.0.19 (`nrfmicro` 0, `sweep-v2.2` 9, `corne-cherry` 79,
`tinytapeout-demo` 30, `buspirate5-rev10` 0, `lumenpnp-mobo` 0, `cynthion` 24,
`glasgow-revC3` 41 for 100, `glasgow-revD1` 437 for 433).

The price at thirty, beside a thousand, four orders at a minute with the rest as it is:
`corne-cherry`, two layers, 1, 0, 2 and 1 open with a median of 506 vias and 7590 mm of
track, for 2, 1, 0 and 0 with 566 and 8269 mm. At twice the pitch, on four layers, it
has less copper too and leaves more open: `glasgow-revC3` 6, 8, 10 and 9 with 396 vias
and 4348 mm, for 11, 4, 5 and 7 with 407 and 4496 mm, and 33 to 36 over four orders in
three such comparisons for 22 to 27; `tinytapeout-demo` 0, 1, 1 and 4 for 0, 3, 1 and 0.
So it is thirty at the first pitch alone. At 40 million nodes, where `corne-cherry`'s
negotiation is cut before the price is there, the boards of two layers are the same
with both.

Shorter runs, on the first run of `tinytapeout-demo` as cleanup begins: 6033 mm, and
5704, 5651 and 5602 mm after each of three times through the nets, two seconds a time;
of 288 runs searched for, 124 came back cheaper, by 442 mm in all, 83 the same, 51
dearer and 30 not at all. `lumenpnp-mobo` 6769 mm and 6641 after the first time, with a
millimetre more from the second. Behind cadlab's adapter, which asks for it in a run of
its own once its other runs are over: 201 to 372 mm and 4 to 15 vias less on
`tinytapeout-demo`, 135 to 141 mm and 28 vias on `glasgow-revC3`, 37 to 93 mm on
`lumenpnp-mobo`.

Tracks between pads that a pour joins, taken out by cadlab's adapter by what
`needless_between` tells and its own fill without them: `lumenpnp-mobo` 569 to 809
tracks and 513 to 652 mm, `corne-cherry` 430 to 457 mm, `cynthion` 311 to 319 mm,
`buspirate5-rev10` 240 mm, `tinytapeout-demo` 212 to 255 mm, `glasgow-revC3` 135 to
143 mm; a pad in a hundred or so is left short of a spoke by the first fill, and has
its tracks back.

### Tried and not kept

- **Ways out for the first ring too**, straight out of each ball: `glasgow-revC3` 17,
  14, 17 and 11 open in two orders of two variants, for 16 and 13 without.
- **Shorter runs at the end of each run**, the first among them. `nrfmicro`'s last
  connection, which the run after the first closes in one order, had no way in five
  runs after a first one whose routes were made shorter (58 s and one open, for 19 s
  and none); `lumenpnp-mobo`'s first run ended 4 to 7 s later and left no time for the
  run that closes its last connection (2, 2, 0 and 1 open for none); `glasgow-revD1`
  had 471 connections open over four orders for 428, with 8800 to 8962 mm of track for
  9064 to 9439. So a host asks for it once, in its last run.
- **A share of cleanup's time by option, a fifth for a twelfth**: no gain that the
  first run's time did not cost the runs after it.
- **A via at its routing price in a run that is searched for again**: `lumenpnp-mobo`
  ended with 13 to 15 vias more, a millimetre saved each; at three times, 3 more to
  one fewer, and `tinytapeout-demo` 6 to 15 fewer for 4.
- **The history at twice its step with the price at a hundred**: `tinytapeout-demo` 372
  and 383 vias in two orders for 321 and 318.
- **Every net after the first time through**, in place of those near a run that moved:
  two seconds a time for a millimetre on `lumenpnp-mobo`.

### As released

0.0.20 from crates.io behind cadlab's adapter (cadlab's D116), which takes the tracks
that `needless_between` tells out in a step of its own after the vias, by its fill and
by the reliefs of the pads, and asks for `shorten` in a run of its own once its other
runs are over, where they leave the time; beside cadlab's own router, on 2026-11-18
with nothing else on the machine: 60 s a board, four runs of cadlab's router with seeds
1 to 4 on three threads and four orders of `guide`'s nets with three threads for its
negotiation, two runs of each at a time, in the same half hour. Connections open:

| board | layers | cadlab's router: open by run (min / median / max); vias, length, time | guide 0.0.20: open by run (min / median / max); vias, length, time |
|---|---|---|---|
| buspirate-flash-sop | 2 | 0, 0, 0, 0 (0 / 0 / 0); 4, 460.2 mm, 0.5 s | 0, 0, 0, 0 (0 / 0 / 0); 5, 491.9 mm, 0.8 s |
| buspirate-rs232 | 2 | 0, 0, 0, 0 (0 / 0 / 0); 10, 280.9 mm, 0.5 s | 0, 0, 0, 0 (0 / 0 / 0); 4, 358.5 mm, 0.7 s |
| lumenpnp-ringlight | 2 | 0, 0, 0, 0 (0 / 0 / 0); 1, 176.5 mm, 0.4 s | 0, 0, 0, 0 (0 / 0 / 0); 4, 199.3 mm, 1.0 s |
| nrfmicro | 2 | 0, 0, 0, 0 (0 / 0 / 0); 46, 453.6 mm, 11.5 s | 0, 0, 0, 0 (0 / 0 / 0); 47, 530.6 mm, 19.5 s |
| sweep-v2.2 | 2 | 3, 3, 3, 3 (3 / 3 / 3); 21, 2160.9 mm, 51.3 s | 3, 3, 3, 3 (3 / 3 / 3); 18, 2207.0 mm, 58.3 s |
| corne-cherry | 2 | 3, 4, 0, 3 (0 / 3 / 4); 484, 8273.0 mm, 29.2 s | 1, 0, 2, 1 (0 / 1 / 2); 495, 7398.6 mm, 58.6 s |
| tinytapeout-demo | 4 | 15, 3, 15, 2 (2 / 9 / 15); 238, 4930.2 mm, 61.1 s | 0, 7, 0, 0 (0 / 0 / 7); 327, 5465.8 mm, 57.4 s |
| buspirate5-rev10 | 4 | 4, 2, 12, 5 (2 / 4.5 / 12); 314, 3044.6 mm, 30.0 s | 0, 0, 0, 0 (0 / 0 / 0); 296, 3243.4 mm, 28.7 s |
| lumenpnp-mobo | 4 | 13, 9, 5, 13 (5 / 11 / 13); 340, 5370.3 mm, 62.1 s cut | 0, 0, 0, 0 (0 / 0 / 0); 331, 6141.6 mm, 59.2 s |
| cynthion | 4 | 28, 29, 29, 28 (28 / 28.5 / 29); 476, 3176.6 mm, 62.4 s cut | 8, 8, 8, 8 (8 / 8 / 8); 423, 3761.1 mm, 58.7 s |
| glasgow-revC3 | 4 | 36, 37, 35, 31 (31 / 35.5 / 37); 415, 3436.4 mm, 60.4 s cut | 7, 5, 5, 7 (5 / 6 / 7); 453, 4464.6 mm, 59.3 s cut |
| glasgow-revD1 | 6 | 241, 289, 238, 250 (238 / 245.5 / 289); 898, 7744.3 mm, 60.0 s cut | 106, 87, 84, 120 (84 / 96.5 / 120); 1139, 9518.3 mm, 57.9 s cut |
| all twelve | | 343, 376, 337, 335 (335 / 340 / 376) | 125, 110, 102, 139 (102 / 117.5 / 139) |
| the eleven without glasgow-revD1 | | 102, 87, 99, 85 (85 / 93 / 102) | 19, 23, 18, 19 (18 / 19 / 23) |

No DRC error but connections open in the 96 runs, and no call of `guide` over 59.5 s.
`guide` is behind in every run on no board, and ahead in every run on five. Against
0.0.19 a few hours earlier: `glasgow-revC3` 5 to 7 open for 29 to 34, `glasgow-revD1` 84
to 120 for 93 to 112, `corne-cherry` 1, 0, 2 and 1 for 2, 1, 0 and 0,
`tinytapeout-demo` 0, 7, 0 and 0 for 0, 7, 1 and 0, the others as they were. The twelve
boards have 102 to 139 for 142 to 157.

Track, by the medians of the four runs against cadlab's router's, with 0.0.19's in
brackets, and after the semicolon the same for the track outside the pads of its net:
`buspirate5-rev10` 5 % over (17 %); 2 %, `lumenpnp-mobo` 12 % (20 %); 6 %,
`corne-cherry` 8 % under (5 % over); 9 % under, `nrfmicro` 19 % (22 %); 9 %, `cynthion`
18 % (19 %); 9 %, `tinytapeout-demo` 23 % (23 %); 21 %, `glasgow-revD1` 22 % (21 %);
15 %, `glasgow-revC3` 29 % (12 %); 24 %, with some 25 connections more through its
array, `sweep-v2.2` 2 %; 0 %, `buspirate-rs232` 28 %; 7 %, `lumenpnp-ringlight` 12 %;
3 % under, `buspirate-flash-sop` 7 %; 0 %. What went is the track between pads that a
pour joins: `lumenpnp-mobo` 520 to 552 mm in every order, `corne-cherry` 310 to 384 mm,
`buspirate5-rev10` 226 to 250 mm, `tinytapeout-demo` 201 and 223 mm in two orders of
four, `glasgow-revC3` 135 mm in one. And where the runs end before the minute, a
shorter way: `buspirate5-rev10` 75 to 188 mm with up to 17 vias, `tinytapeout-demo`
322 mm and 15 vias in the one order whose first run ended early, `nrfmicro` 0.4 to
6.4 mm.

Vias, the range of the four runs against cadlab's router's, and 0.0.19's in brackets:
`corne-cherry` 473 to 525 against 445 to 505 (522 to 609), 8 % over by the medians for
22 %; `tinytapeout-demo` 327 to 462 against 238 to 290 (362 to 462); `buspirate5-rev10`
296 to 326 against 291 to 314 (308 to 348); `lumenpnp-mobo` 331 to 348 against 339 to
348 (331 to 348); `cynthion` 418 to 423 against 469 to 482 (405 to 430);
`glasgow-revC3` 399 to 453 against 415 to 439 (330 to 359); `glasgow-revD1` 1067 to
1139 against 866 to 932 (1080 to 1114).

Pads in pours short of two spokes, cadlab's count over the twelve boards: 64 to 75 in
the four orders for 73 to 77 with 0.0.19, against 154 to 162 after cadlab's router;
with none, 0 to 3 as before. The tracks taken out between pads leave no pad short that
was not: the host puts back those of a pad that its fill does not relieve without them.

At 40 million nodes the crate gives the 620 that the source gave. At 150 million, two
orders, 0.0.19 in brackets: `corne-cherry` 1 and 0 (2 and 1), `tinytapeout-demo` 0 and
3 (0 and 3), `cynthion` 8 and 8 (8 and 8), `glasgow-revC3` 5 and 9 (24 and 26),
`buspirate5-rev10`, `lumenpnp-mobo` and `nrfmicro` none, `sweep-v2.2` 3 and 3; there the
host makes both of its steps on every board that the budget does not stop, and
`tinytapeout-demo` has 5690 and 5432 mm of track with 358 and 341 vias, 16 % and 10 %
over cadlab's router's median at a minute, `lumenpnp-mobo` 6064 and 6013 mm, 10 % and
9 %, `cynthion` 3558 and 3471 mm, 12 % and 9 %. The curated boards, with `whole` and
`shorten` off and no host: 1150 of 1151 connections with 712 vias and 37556357 nodes,
as before.

In cadlab's adapter, not here, and found by this:

- With `shorten` in every run, the first among them, `nrfmicro`'s last connection had
  no way in five runs after it and `lumenpnp-mobo` had no time for the run that closes
  its last; so it is asked for once, in a run of its own after the others.
- With time kept for that run, 6 % of the minute and four verifications, the boards
  that a minute does not finish had no run that moves routes: `tinytapeout-demo` 13 and
  16 open over four orders for 6, `lumenpnp-mobo` 4 and 5 for none, `glasgow-revC3` 36
  for 22. So no time is kept, and those boards have their shorter ways from a larger
  budget.
- A pour may be cut by a shorter way without `guide`'s count of its fills telling:
  two pads of `cynthion`'s +2V5, under its array, in every run. The host's check after
  the run tells it, and the nets with new copper beside those pads keep their old
  routes.
- Routes that were given for a pair's nets or for a net with a length to keep were
  made shorter with the rest: the pair came apart and the meanders went. `shorten`
  leaves those nets as they are since.

### What this does not show

- An array that two tracks pass between two balls of, or one with balls left out in
  its middle rows: the corpus has neither, and the ways out are planned for one track.
- Shorter ways on a board that a minute does not finish. cadlab's adapter keeps no time
  for them: with 6 % of the minute and four verifications kept, `tinytapeout-demo` had
  13 connections open over four orders for 6, `lumenpnp-mobo` 4 for none and
  `glasgow-revC3` 36 for 22, for 5 %, 1 % and 3 % of their track.
- Tracks that end at the edge of a pad. Counted outside the pads of their nets,
  `guide`'s track is nearer cadlab's router's than by its whole length, and no shorter.

## PCB routing from routes, with none moved

What was changed after [the release above](#pcb-routing-out-of-an-array-and-shorter-after-it),
0.0.20, for the same host, cadlab, which makes its last run a run for shorter ways: the
routes of every run before it given back, nothing to route, no rip-up
([decisions](architecture.md#decisions)). Measured on 2026-11-18 through cadlab's
adapter and its `route_bench`, with a budget of nodes, one thread, so that two versions
of the code are compared on the same searches.

### What

`cynthion`'s run for shorter ways came back, in every run of it, with two pads of a
supply open that were joined before it, and the host kept the routes it had. Looked at
on the board: the supply's via between four balls of the array was gone, with the three
tracks that met at it. It was not in the world's way when the first run made it. In the
run from routes it was, by a track and a via of the ball beside it, another supply's:
that ball's dog-bone, planned as the run began on a board without the routes, in the
place where a first run had planned it, had not used it, and had given it to the other
net since. The same happened in the runs that move routes, where it cost time and no
connection: 74 to 78 nets of 334 were routed again in each of them, 16 to 22 of them
moved aside.

A run with `rip_ups` at none now plans its escapes on the board with the routes it was
given on it.

### Results

`cynthion`, 150 million nodes, one thread, the host's adapter otherwise the same:

| | nets routed again in the run for shorter ways | connections open after it, for 8 before | kept by the host | vias at the end | track at the end |
|---|---|---|---|---|---|
| 0.0.20 | 7, 2 of them with no way | 11 | no | 426 | 3558 mm |
| escapes round the routes, in a run that moves none | 4 | 8 | yes, 97 mm less | 424 | 3449 mm |

### Tried and not kept

- **Escapes round the routes in every run from routes.** The runs that move routes then
  route 12 nets again where they routed 74 to 78, and close less: over four orders at
  150 million nodes `glasgow-revC3` had 7, 9, 7 and 6 connections open for 6, 9, 4 and
  3, `tinytapeout-demo` none either way and `corne-cherry` 1, 0, 2 and 0 either way. An
  escape planned over another net's route is how the ball it is for gets out.
- **A negotiation for every net ended when it has stopped gaining**: once six rounds
  have passed since the points shared last fell by a tenth, in place of twelve since
  the fewest, with the best round's routes kept for what follows. `tinytapeout-demo`'s
  has 3335 points shared after its first round, 113 after its eleventh, and between 49
  and 230 for the 26 rounds after it, in which its vias go from 381 to 425; ended
  sooner, the passes after it had more to do than the rounds cost. Over four orders at
  150 million nodes, the best round's routes kept either way: `tinytapeout-demo` 4
  connections open for 2, `corne-cherry` 10 for 5, `glasgow-revC3` 25 for 24; by the
  medians `tinytapeout-demo` 364 vias for 341 and `glasgow-revC3` 377 for 392, the
  others within three in a hundred.
- **The searches for fewer vias left to a host's last run by option**, where every run
  has them. Where that run is made the board ends within two in a hundred of the same
  track and vias, and where the time leaves none for it the vias of every run stay:
  `cynthion` 446 at a minute for 423, by the medians of four orders.

### As released

0.0.21 from crates.io behind cadlab's adapter (cadlab's D117), which also refuses the
fill that a run asks for at its end, ends its runs that move routes after two that say
the board has no room, and fills its zones faster; beside cadlab's own router, which has
that fill too, on 2026-11-18: four runs of each at 60 s, three threads each, two of each
at a time.

- Open, the twelve boards: 102 to 140 for `guide` against 178 to 268; ahead in every run
  on `buspirate5-rev10`, `lumenpnp-mobo`, `cynthion` and `glasgow-revC3` (4 to 7 against
  31 to 36), level on five, ranges that overlap on `corne-cherry`, `tinytapeout-demo`
  (0, 6, 0 and 1 against 2 to 14) and `glasgow-revD1` (84 to 120 against 99 to 169, where
  cadlab's router had 238 to 289 with a fill of 3 s), behind on none.
- The run for shorter ways is begun in 28 runs of 48 for 23 with 0.0.20, and kept in 27:
  on `lumenpnp-mobo` in every order (72 to 127 mm) and on `cynthion` in two, for none.
  `cynthion`'s track is 14 % over cadlab's router's by the medians for 18 %, and at
  150 million nodes 3449 and 3398 mm in two orders for 3558 and 3471.
- At 40 million nodes, three orders, the nine boards a budget cuts: 620 open, board by
  board what 0.0.20 had.
- `tinytapeout-demo` and the two `glasgow` boards end their first run at its share of
  the minute, in every order, and have what is left for everything after it:
  `tinytapeout-demo` 372 to 427 vias against 243 to 290 and 20 % more track,
  `glasgow-revC3` 30 % more. `glasgow-revD1` with three minutes has 23 connections open
  and with five 14, all at its array of 256 balls.

### What this does not show

- A run that moves routes without the nets cut as it begins: the escapes planned again
  are what it has for the balls left shut in, and no other way to those was tried.
- More than one thread, or a deadline: the comparison is by nodes.

## PCB routing with what the routes cut off the fills joined to them past a deadline

0.0.41 left one run of `tinytapeout-demo` in sixteen orders with three connections open
beside cadlab's own router at 60 s, and the round after it began with that run: what a run
that its deadline ends returns. **One thing is changed. The round that joins the pads that
the routes have cut off their fills to them again, once nothing moves any more, was begun
only with time left and ended at the deadline between two of its searches, or in one: a run
whose passes had all of its time returned such pads as they were, and counted their
connections as made, on the fill that it had planned on. The first such round is now made
whatever the clock says, each search with the nodes it may expand, and only the caller's
flag or a budget of work ends it; a run with no deadline is what it was, to the node. It
does not close the three connections: those, and nearly every connection that
`tinytapeout-demo` is left with, are pads that no search joins among the routes as they
are, whether the deadline cut the run or not.**

### What a run that its deadline ends returned, and returns

A run's time is shared out: routing, then cleanup with what routing left
([`RouteOptions::whole_share`], [`RouteOptions::join`]). Before cleanup, each net is laid
again where the others are, and then, in up to three rounds, every net with a fill is asked
for the pads and stubs that the routes cut off it (by the host's own fill around the routes,
where a host gives one) and a search joins each to the fill again by stubs that move
nothing. Every step of that looked at the clock first, and a search under way took the
run's stop flag.

`a_run_that_its_deadline_ends_joins_a_pad_cut_off_its_fill_all_the_same` (`pcb::pours`) is
the case in small: a strip of fill with a pad behind another net's track, a host that
answers the round's request for its fill when the deadline is past. 0.0.41 returned the
board with the pad on its own piece, `stopped: Deadline` and the net among its `unrouted`;
this returns it joined. `Run::overtime` is what makes the round: while it lasts the clock
ends no part and stops no search, the caller's flag stops everything as it did and the run
is then one that its caller stopped, and a budget of work that runs out ends it too
(`what_a_run_does_past_its_deadline_is_stopped_by_its_caller_and_its_budget_alone`).

On a board (`tinytapeout-demo` through cadlab at 60 s, the order whose run has three open in
every set measured; a trace of the stages that is in no release): the rounds of cadlab's
router in this crate end at 27.2 s with seven nets to route, the passes take 246 nets out
for them, are ended by their share at 45.2 s and go back to the routes that made the most,
two nets open; the round begins at 45.8 s with the host's fill, finds three pieces of the
ground's fill and one of a supply's cut off, and the deadline is at 46.8 s. 0.0.41: the
round ended there with the first net's search cut, the run was back at 46.9 s with four
nets open on the host's filled board, and the host's second run, on that board with the
routes fixed, made the ground's stubs in 1.5 s. This: the search goes its way, 45 segments
and two vias for the ground's three pieces by 47.65 s, the supply's pad has no way, and the
run is back at 47.8 s with three nets open. The board leaves with the same three
connections open either way, at 51.8 s for 56.5 s.

### What `tinytapeout-demo` is left with

Sixteen orders at 60 s, this source and 0.0.41 side by side in the same minutes (four runs
at a time on three threads each, the load 4.4 at the start and 4.5 at the end; open is the
host's DRC's count on the board that left): 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 0, 0, 3, 0, 0 and 0
for 0, 0, 0, 0, 0, 0, 1, 1, 1, 2, 0, 0, 3, 0, 0 and 0; medians of 276.5 vias and 4848 mm for
287 and 4867 mm. Six connections for eight in sixteen runs is not told from noise, and the
run with three is the same run.

What those connections are, by the trace over three sets of sixteen orders: in 6 to 8 first
runs of 16 a pad of a net with a pour, or a via of its stubs, is on a piece of its own when
the rounds of the joining end, and the search for it comes back with nothing: its way out is
between the tracks of its neighbours on a package of 0.4 mm pitch (`U6.68 is shut in by pad
U6.67, pad U6.70, track, track`). It is so in runs that end on their own, with eight
seconds of the minute unused, as in those that the deadline cuts. The host's run on the
filled board with the routes fixed finds no way for it either; a run that has every route
back to move takes 137 to 195 nets out for the one pad and closed it in none of seven
tries, in 3 to 12 s each. And the order with three: two of its three are the nets that
the passes left open, each shut in the same way.

### Budgets of nodes

One thread, this crate alone behind cadlab's adapter, beside 0.0.41's sets. At 40 million
nodes, nine boards in three orders: every one of 27 runs has the connections open, the
vias and the track that it had (570 open, 9997 vias, 109 321 mm). At 150 million, seven
boards in two orders: every one of 14 likewise. A run with no deadline has no overtime.

### Tried and not kept, and what remains open

- *A round of the joining after every cleanup.* Cleanup's own steps, each net pulled tight
  and laid again for fewer vias, cut a pad off its fill in about one first run of
  `tinytapeout-demo` in four (the host asked for its fill after cleanup, sixteen orders:
  six runs with a pad cut off then, in most of them the pad that the rounds before cleanup
  had left so, in two one more). A round after cleanup joined such a pad in 4 runs of 16 and saved
  the host its second run there; it asks the host for a fill more in every run that has
  cleanup, and `cynthion`'s first run was back 2.4 s later (19.9 s for 17.4 s) with the
  board it had. Over four orders no connection, via or millimetre differs for it on
  `buspirate5-rev10`, `cynthion`, `lumenpnp-mobo`, `sweep-v2.2` and `corne-cherry`. Not in
  the release: after a cleanup that looked for shorter ways the
  round is made as it was, with the time for it.
- *A run that moves only the routes near what is open*, in cadlab's adapter: the nets with
  copper within 2 mm of an end of an open connection handed back, the others fixed. Five
  tries on `tinytapeout-demo` closed one pad, in 1.5 to 7 s each with up to 73 nets taken
  out among the 14 to 51 handed back; `corne-cherry`'s first order spent 12 s on one, 119
  nets taken out among 17, for nothing. Not kept.
- *Chains of moves for the pieces cut off* are what settling does not do, by an earlier
  measurement (`settle.rs`): not tried again.
- **What remains open**: a pad of a net with a pour that the routes of the rounds shut in.
  Nothing that runs after the rounds joins it; what would is a way kept for it while the
  nets round it are still negotiated, which the rounds' three looks at the pads cut off do
  for the pads they see, and not for the one that the last routes made legal cut off.

## PCB routing with a run pulled tight no nearer a pad across the gap of its spoke

0.0.40 left eight cells worse beside cadlab's own router at 60 s, and the round after it was
for the track and the vias of the two `glasgow` boards and of `tinytapeout-demo`. **Two
things are changed. Pulling a net's runs tight, the step of cleanup that takes no search,
laid a run that went round the end of another net's thermal spoke straight across the gap
that the spoke bridges, and the pad lost the spoke: a way is now refused where it is in the
room of a spoke of another net's pad and nearer the place where the spoke starts in its pad
than the run it stands for. And after this crate's own rounds, with no zones to join,
cleanup has a quarter of the time that a deadline leaves when the passes begin, for a
twelfth. `glasgow-revD1` through cadlab at 60 s, beside 0.0.40 in the same minutes: 10, 12,
14 and 10 pads short of two spokes for 8, 23, 23 and 22, 1122 vias for 1145.5, 10 514 mm of
track for 10 576 mm, and 10, 17, 8 and 13 connections open for 12, 17, 4 and 12. Neither the
track nor the vias of the boards come within what cadlab's router
has on the nets that both complete, and what was measured says that time does not give it.**

### Where `glasgow-revD1`'s track is

Through cadlab at 60 s, 0.0.40, by the nets that both routers complete (495 to 503 of 561):
7753 mm against 7163 mm in the default order, +590 mm, of which the nets that are longer
with this crate carry +852 mm and those that are shorter −261 mm. The five nets with the
most carry +186 mm, ten +287, twenty +423, fifty +618: it is spread, with a few long ways
round (one net 87 mm for 18 mm). With 120 s the first run is ended by its deadline again,
at 84 s, with 854 nets taken out for 332, and the nets both complete have +440 mm (+6 %);
with 240 s it ends on its own at 106 s, cadlab's run for shorter ways has its turn and takes
84 mm and 6 vias off, and they have +337 mm (+5 %) with 5 connections open for 12. cadlab's
router leaves 105 to 144 connections open on this board, and the nets it completes have the
room of those.

### What the first run's cleanup had, and what it has

The stages of the first run, by a trace that is in no release, 0.0.40, four orders: the
rounds end at 23 to 27 s, the passes at 40.5 to 40.8 s of a deadline at 42 s. In two orders
the joining of what the routes cut off the fills, 1.1 s, ended at the deadline and nothing
of cleanup was begun; in the two others every net was pulled tight, 0.07 s for 290 to
330 mm of 11 700, and the searches for fewer vias were cut as they began. Each net routed
again where the others are, which wants a second of its own, had its turn in none.

| cleanup's share of the time | open, four orders | vias | track | pads short of two spokes after the first run |
|---|---|---|---|---|
| a twelfth (0.0.40) | 12/17/7/12 | 1145.5 | 10576 mm | 8/23/10/22 |
| a quarter | 12/17/7/12 | 1130.5 | 10499 mm | 20/23/25/21 |
| two fifths | 12/17/7/12 | 1124 | 10417 mm | 20/23/23/22 |
| a quarter, and the rule for the spokes (0.0.41) | 12/17/7/12 | 1130 | 10515 mm | 10/12/14/11 |

The passes end with the connections they had with less of the time: what they are given
past a quarter of it is nets taken out and put back. `glasgow-revC3` has its cleanup whole
with a twelfth already (each net again in 0.4 s, the tightening in 1.1 s, the searches for
fewer vias in 1.7 s), and 402, 395 and 394 vias with the three shares, 9/6/9/8, 8/6/9/8 and
12/6/9/8 connections open. After the rounds of cadlab's router the share is as it was:
`tinytapeout-demo` had 3 connections open in two runs of twelve with the quarter and in none
of 32 with the twelfth, its passes having a few nets to settle in what the run has left.

### The pads that pulling tight left short of their spokes

The two orders that pulled nothing tight had 8 and 10 pads short of two spokes after the
first run, the two that did 23 and 22; with the tightening alone, and nothing else of
cleanup, 20 and 23 for 8 and 12 without it. Twelve pads of the default order are short with
it and not without, each the ground pad of a decoupling capacitor beside a row of pins: a
track from a pin to a resistor went 1.06 to 1.26 mm from the pad's centre, round the end of
its spoke, and pulled straight it is 0.76 mm from it, across the gap.

- *Walls round every free room of a spoke for the time of cleanup* (`World::keep_spokes`
  with 2 and with every room), and *the rooms 0.3 and 0.5 mm longer*: 21/23/23/22, 20 and
  23, 20 to 23. The run round the end of a spoke is in its room already, nearer the spoke
  than the clearance, and the room is occupied before and after.
- *What is kept*: a tightened way is refused where it is nearer a spoke of another net's pad
  than the clearance and nearer the place where the spoke starts in its pad than any of the
  run that it stands for (`World::nearer_a_spoke`, of `Zones::spokes`). 10/12/14/11 pads
  short, with 16 mm more track than the tightening without it.

### Budgets of nodes

One thread, twelve runs at a time, beside 0.0.40 (the load 3 to 8). At 40 million nodes, nine
boards in three orders: every run has the connections open that it had, by the run's count
and by the DRC's (570 in all); 9997 vias for 10012 and 109 321 mm for 109 294 mm (+0.02 %);
ten runs of 27 are the same to the via and the tenth of a millimetre, `corne-cherry`,
`glasgow-revC3` and `nrfmicro` in every order. At 150 million, eight boards in two orders:
the connections open of every run as they were (42 on seven boards, 56 and 75 on
`glasgow-revD1`), 6777 vias for 6788, 76 779 mm for 76 709 mm (+0.09 %). The most that a
run differs by: `sweep-v2.2`'s second order, 18 vias for 20 and 2149 mm for 2117 mm;
`tinytapeout-demo`'s default order at 40 million, 263 vias for 277.

### At 60 s, beside cadlab's router measured in the same minutes

The release's source behind cadlab's adapter, cadlab's own router and, on six boards, 0.0.40,
side by side, four runs at a time on three threads each (the load 6.4 at the start and 6.2
at the end). Open is the DRC's count of unrouted on the board that left, for both; open and
the pads short of two necks by order; medians of the vias, the track and the track outside
the pads; the fastest and the slowest run.

| board | open | vias | track | track outside pads | short of two necks | time |
|---|---|---|---|---|---|---|
| `buspirate-flash-sop` | equal (0/0/0/0 against 0/0/0/0) | equal (+0 %) 4 against 4 | better (-4 %) 442 against 461 | equal (-0 %) 431 against 431 | equal (0/0/0/0 against 0/0/0/0) | equal (1 to 1 s against 0 to 0 s) |
| `buspirate-rs232` | equal (0/0/0/0 against 0/0/0/0) | better (-20 %) 8 against 10 | equal (+1 %) 284 against 281 | equal (+1 %) 275 against 272 | equal (0/0/0/0 against 0/0/0/0) | equal (0 to 0 s against 0 to 0 s) |
| `lumenpnp-ringlight` | equal (0/0/0/0 against 0/0/0/0) | equal (+0 %) 3 against 3 | better (-7 %) 166 against 177 | equal (-1 %) 158 against 159 | equal (0/0/0/0 against 0/0/0/0) | equal (0 to 0 s against 0 to 0 s) |
| `nrfmicro` | equal (0/0/0/0 against 0/0/0/0) | better (-14 %) 41 against 47.5 | better (-3 %) 437 against 450 | equal (+2 %) 421 against 414 | equal (0/0/0/0 against 0/0/0/0) | equal (2 to 14 s against 2 to 12 s) |
| `sweep-v2.2` | equal (3/3/3/3 against 3/3/3/3) | better (-20 %) 20.5 against 25.5 | equal (-2 %) 2120 against 2159 | equal (-1 %) 2109 against 2122 | better (0/0/0/0 against 2/1/2/1) | better (36 to 36 s against 53 to 53 s) |
| `corne-cherry` | better (1/0/0/1 against 1/3/2/4) | better (-7 %) 429 against 459 | better (-8 %) 7482 against 8103 | better (-6 %) 7411 against 7915 | better (2/1/1/1 against 3/3/2/2) | worse (27 to 56 s against 21 to 33 s) |
| `tinytapeout-demo` | overlap, median 0 against 3 (1/0/3/0/0/0/0/0 against 19/1/4/5/1/2/2/9) | equal (-0 %) 272 against 273 | equal (-1 %) 4851 against 4894 | worse (+3 %) 4796 against 4672 | better (1/2/1/0/1/3/0/3 against 20/12/10/16/16/14/17/19) | equal (40 to 54 s against 35 to 60 s) |
| `buspirate5-rev10` | better (0/0/0/0 against 4/2/4/4) | better (-6 %) 284.5 against 302 | better (-8 %) 2886 against 3129 | better (-4 %) 2802 against 2911 | better (8/10/8/10 against 22/24/30/21) | better (15 to 19 s against 20 to 33 s) |
| `lumenpnp-mobo` | better (0/0/0/0 against 9/10/7/13) | better (-2 %) 331 against 338.5 | equal (-2 %) 5294 against 5393 | worse (+2 %) 5218 against 5108 | better (0/0/0/0 against 2/1/0/2) | better (30 to 33 s against 61 to 62 s) |
| `cynthion` | better (8/8/8/8 against 28/29/30/28) | better (-13 %) 416.5 against 478 | better (-5 %) 3037 against 3184 | equal (-1 %) 2963 against 2981 | equal (1/1/1/1 against 1/1/1/1) | better (32 to 37 s against 62 to 62 s) |
| `glasgow-revC3` | better (8/6/9/7 against 35/34/39/30) | better (-6 %) 395.5 against 419.5 | worse (+5 %) 3632 against 3448 | worse (+11 %) 3542 against 3190 | equal (0/0/0/0 against 0/0/0/0) | equal (58 to 59 s against 61 to 62 s) |
| `glasgow-revD1` | better (10/17/8/13 against 127/109/119/144) | worse (+6 %) 1122 against 1059.5 | worse (+17 %) 10514 against 8968 | worse (+21 %) 10261 against 8447 | better (10/12/14/10 against 43/31/43/47) | better (54 to 56 s against 60 to 64 s) |

Of 72 cells 33 are better, 30 equal, one has ranges that overlap and 8 are worse, where
0.0.40 had 36, 28, none and 8. What moved:

- *`glasgow-revD1`*: 10, 12, 14 and 10 pads short of two necks for 8, 23, 23 and 22 with
  0.0.40 in the same minutes, 1122 vias for 1145.5 (+6 % beside cadlab's router for +8 %),
  10 514 mm for 10 576 mm, 54 to 56 s for 56 to 58 s, and 10, 17, 8 and 13 connections open
  for 12, 17, 4 and 12. `glasgow-revC3`: 395.5 vias for 402, 8, 6, 9 and 7 open for 9, 6, 9
  and 8.
- *`sweep-v2.2`'s track*, equal for better: 2120 mm for 2115 mm against 2159 mm, either side
  of two in a hundred, with no pad short of two necks in any order for one in two orders.
- *`tinytapeout-demo`'s connections open*, ranges that overlap for better: one order of the
  eight ends with 3 open, its first run ended by its deadline before what the routes cut off
  the fills was joined to them again. Over sixteen orders 1, 0, 3, 0, 0, 0, 0, 0, 1, 2, 2, 0,
  1, 0, 0 and 0, where 0.0.40 had 0, 0, 0, 0, 0, 0, 1, 0, 1, 2, 0, 0, 3, 0, 0 and 0: the same
  kind of run, in another order. Its vias, equal for worse: 272 against 273, with 0.0.40 at
  280.5 in the same minutes and the runs of one order 259 to 303 apart; not this release's.
- *`lumenpnp-mobo`'s track and track outside the pads*, equal for better and worse for
  equal: this crate's four boards are within 4 mm of what they were (5294 mm, 5218 mm
  outside the pads), and cadlab's router has 5393 mm and 5108 mm for 5446 mm and 5160 mm.

The worse cells: `corne-cherry`'s time; `tinytapeout-demo`'s track outside the pads;
`lumenpnp-mobo`'s track outside the pads; `glasgow-revC3`'s track and track outside the
pads; `glasgow-revD1`'s vias, track and track outside the pads. In the 60 runs of this
crate, sixteen orders of `tinytapeout-demo` among them, no pad of a pour is without a spoke,
none is made solid by a track, the DRC has no error other than unrouted, and the run's count
of open connections is the DRC's in every one.

The same runs by the nets that both routers complete:

| board | nets both complete (median of the pairs) | vias on them | track on them | track outside pads on them | nets only guide completes, their track and vias; only native |
|---|---|---|---|---|---|
| `buspirate-flash-sop` | 12 | equal (+0 %) 4 against 4 | better (-4 %) 442 against 461 | equal (-0 %) 431 against 431 | 0 nets, 0 mm, 0 vias; 0 |
| `buspirate-rs232` | 30 | better (-20 %) 8 against 10 | equal (+1 %) 284 against 281 | equal (+1 %) 275 against 272 | 0 nets, 0 mm, 0 vias; 0 |
| `lumenpnp-ringlight` | 12 | equal (+0 %) 3 against 3 | better (-7 %) 166 against 177 | equal (-1 %) 157 against 159 | 0 nets, 0 mm, 0 vias; 0 |
| `nrfmicro` | 41 | better (-14 %) 41 against 47.5 | better (-3 %) 437 against 450 | equal (+2 %) 421 against 414 | 0 nets, 0 mm, 0 vias; 0 |
| `sweep-v2.2` | 40 | better (-23 %) 18.5 against 24 | equal (-1 %) 2039 against 2060 | equal (+0 %) 2029 against 2028 | 0 nets, 0 mm, 0 vias; 0 |
| `corne-cherry` | 149.5 | better (-5 %) 423 against 446 | better (-8 %) 7324 against 7991 | better (-7 %) 7256 against 7807 | 2.5 nets, 158 mm, 12 vias; 0.5 |
| `tinytapeout-demo` | 136 | worse (+6 %) 247.5 against 233 | equal (+2 %) 4758 against 4684 | worse (+5 %) 4708 against 4499 | 1 nets, 58 mm, 23.5 vias; 0 |
| `buspirate5-rev10` | 181 | equal (+1 %) 251.5 against 250 | better (-2 %) 2869 against 2927 | worse (+2 %) 2788 against 2720 | 2 nets, 28 mm, 33.5 vias; 0 |
| `lumenpnp-mobo` | 188.5 | equal (+2 %) 264 against 259.5 | better (-3 %) 4988 against 5121 | equal (+1 %) 4922 against 4865 | 4.5 nets, 255 mm, 64 vias; 3.5 |
| `cynthion` | 310.5 | better (-23 %) 276.5 against 360.5 | better (-6 %) 2695 against 2870 | better (-3 %) 2635 against 2714 | 15.5 nets, 348 mm, 134.5 vias; 0 |
| `glasgow-revC3` | 199.5 | better (-18 %) 210 against 257.5 | equal (-0 %) 2667 against 2675 | worse (+4 %) 2613 against 2509 | 19 nets, 938 mm, 184 vias; 6.5 |
| `glasgow-revD1` | 500.5 | better (-7 %) 739.5 against 791 | worse (+7 %) 8096 against 7566 | worse (+10 %) 7902 against 7168 | 50 nets, 2418 mm, 374.5 vias; 10 |

Over these 36 cells 16 are better, 14 equal and 6 worse, as with 0.0.40; `lumenpnp-mobo`'s
vias are equal for worse and `buspirate5-rev10`'s track outside the pads worse for equal,
both at two in a hundred.


### Tried and not kept, and what remains open

- *A net's route with fewer vias across the routes of at most two other nets, those laid
  again round it*: built, and kept for none of 91 nets of `tinytapeout-demo` at 150 million
  nodes. With vias ten times as dear and a route crossed at a quarter of a via a step, 67
  nets have the vias they had with no route crossed, and where a way with fewer crosses
  routes it crosses 4 to 12 nets: a net that goes under and back goes under a bundle. Not
  in the release.
- *The share of the rounds of cadlab's router* at 0.5, 0.65 and 0.8 of the run on
  `tinytapeout-demo`: 280, 280.5 and 280.5 vias over four orders. The rounds end on their own
  in most runs with the half.
- *What is open after those rounds settled before any net is taken out*, where eight nets or
  fewer are: 275.5 vias for 278 over six and eight orders, not told from noise. The rounds
  of one order end with 6 nets to route in one run and 7 in the next, and the runs whose
  passes take two nets out or none end with 259 to 281 vias, those that take 27 to 148 out
  with 288 to 294.
- *cadlab's run for shorter ways on `corne-cherry`*, by stage: 4.2 s here in an order whose
  run ends at 28 s, of which each net again 1.9 s (6 of 326 kept, 18 mm and 2 vias), the
  shorter runs 1.4 s (20 mm of the 1107 mm that 111 nets' runs cost over straight ways) and
  the tightening after them 0.2 s (19 mm); `cynthion` 7.4 s for 53 mm of 3473 and 7 vias.
  Not gated: nothing that the stage knows before it tells the one from the other.
- *The step that moves routes out of the rooms of spokes on `glasgow-revD1`* is not kept in
  any of 12 runs measured (it leaves 1 to 7 connections more open), in 6 s of each.
- *`glasgow-revD1`'s and `glasgow-revC3`'s track on the nets both complete*: as above.

## PCB routing with cleanup's searches made ahead on the other threads

0.0.39 left nine cells worse beside cadlab's own router at 60 s, and the round after it was
to be one of speed with no result changed: the search of the passes, the fills, the threads,
and what the host does between the runs. **Two things are changed, and neither changes a
route. In a run with no budget of nodes, a net's search for a route with fewer vias is made
ahead of its turn on the other threads, in a copy of the board as it is, and stands at the
turn if no net before it has ended with other copper since, which 92 to 97 nets in a hundred
have not; and the nets of a round that are routed at once are begun the longest first. With
an hour for a budget, which no run comes near, eight boards have on three threads the
connections open, the vias, the track and the nodes that they have on one; at budgets of
nodes every one of 43 runs is 0.0.39's. The search of the passes itself is not faster: four
changes to it were measured and none is told from noise.**

### Where the minute goes, by stage

Through cadlab at 60 s on three threads, 0.0.39; the time on the clock and the processor
time of the whole process in each stage (`CADLAB_BENCH_TRACE=cpu` in cadlab's `route_bench`
adds the processor time to each line of the trace), medians of four orders, the load 4.3 to 7.7 (each cell the time on the clock, then the
processor time):

| stage | `corne-cherry` | `tinytapeout-demo` | `glasgow-revD1` | `cynthion` |
|---|---|---|---|---|
| the board looked at, the escapes and the planes | 1.5 s; 1.6 s | 3.2 s; 3.5 s | 4.4 s; 4.9 s | 5.6 s; 6.1 s |
| routing: the rounds, and the passes that take nets out | 23.0 s; 34.1 s | 33.2 s; 33.6 s | 39.5 s; 52.5 s | 4.0 s; 4.4 s |
| settling what is open | 8.6 s; 9.1 s | 0.1 s; 0.1 s | 0.0 s; 0.0 s | 1.1 s; 1.1 s |
| cleanup | 8.2 s; 8.7 s | 10.7 s; 12.1 s | 0.4 s; 0.4 s | 15.8 s; 19.1 s |
| cadlab between the runs: its check, its fill, the plan of the next | 0.9 s; 1.5 s | 1.3 s; 2.6 s | 1.9 s; 4.0 s | 3.4 s; 8.0 s |
| cadlab after the last run: what the fill makes needless, the spokes, the ends in the pads | 2.1 s; 3.8 s | 2.9 s; 5.9 s | 9.3 s; 16.8 s | 5.5 s; 13.6 s |
| the whole run | 45.4 s; 60.5 s | 51.7 s; 59.0 s | 57.7 s; 83.8 s | 37.8 s; 58.7 s |

A stage with as much processor time as time on the clock is one thread at work. This crate's
own rounds, which are most of the routing of `corne-cherry` and `glasgow-revD1`, have 1.3 to
1.5 threads' worth on three; the rounds of cadlab's router in this crate, which are the
routing of `tinytapeout-demo`, have one, their batches being of one net; what settles the
open connections and what cleans up have one, but for cadlab's fills asked for within them;
cadlab's own steps, between the runs and after the last, have about two. So the minute is
mostly one thread, as the release before found by the processor time alone, and the stages
say where.

### What a node of the passes costs

A run at a budget of nodes is mostly the passes, and they expand 0.4 to 0.8 million nodes a
second over a whole run where this crate's rounds expand 3.7 to 5: the question was what a
node of the passes has that a node of the rounds has not. Counted on `glasgow-revD1` at 40
million nodes (`corne-cherry` and `tinytapeout-demo` within a fifth of it), for each node
expanded: twelve numbers of nodes asked for, three of them by a hash of a block and one and
a half by a hash of a node off the lattice; four pads of the net looked at for being near,
of which 1.9 are; 0.47 questions to the spatial index for what is near the node, which find
1.8 entries; 0.26 tracks that are no step of the lattice, each a question of its own; 0.12
vias. The rounds search a grid of cells whose claims are numbers in an array; the passes ask
exact geometry of an index of shapes at half of their nodes, and number their nodes as they
meet them. That is what the factor is made of, and it is spread: no one of these is a third
of a node.

Four changes that change no result were made to it and measured, each of 43 runs the same to
the node, on four boards at 40 million nodes side by side with the release before:

- *A node's place by comparison with the node that its steps come from*, where it was by two
  divisions, and *the blocks looked up last kept by the low bits of their keys*, 128 of them
  for one;
- *the pads near a node by boxes widened once*, and the cell of the pads kept from one node
  to the next;
- *the two costs of an entry of the heap compared as one number of 128 bits*, as the rounds'
  heap has it since 0.0.39;
- *the sides of a rectangle of the index compared with no branch on each*.

Together: `glasgow-revD1` 40.30 s for 40.47 s in the crate, `corne-cherry` 29.83 for 29.71,
`tinytapeout-demo` 54.41 for 55.18, `cynthion` 33.50 for 33.67. Not told from noise, and none
of them is in the release. The profile by a timer's signal puts 12 to 16 % of the searches'
time on each of these lines; what it samples is where the processor waits for memory, which
the instructions saved do not shorten. `callgrind` with a simulated cache, which would tell,
did not get past global routing in 25 minutes on this board.

### The searches for fewer vias, made ahead

Cleanup routes each net with vias again, with vias ten times as dear, and keeps the route if
it has fewer and is not much longer. Timed by net at 150 million nodes on one thread:

| board | nets looked at in the first run | nets that end with another route | the searches | the same on three threads, at best |
|---|---|---|---|---|
| `tinytapeout-demo` | 92 | 5 | 6.06 s | 2.17 s |
| `cynthion` | 177 | 14 | 3.85 s | 1.68 s |
| `corne-cherry` | 98 | 3 | 2.46 s | 0.98 s |

("At best": the searches between one net that changes and the next shared out over three
threads with nothing lost, and none shorter than its longest.) A search asks the world
without its net's copper, and at 92 to 97 turns in a hundred the world is, but for copper
lifted and laid back where it was, what it was at the turn before. So the world is copied as
it is, each thread makes a world of its own of the copy and takes the nets to come in their
order: the net's copper out, the search, the copper back. A net's turn has the search that
was made for it, charges the run its nodes and goes on as it would with its own. When a net
does end with other copper the searches made ahead are left, those under way too, and the
world is copied again: 1.2 ms on `glasgow-revD1`, and 2.1 ms for a thread's own of it.

That a copy answers as the world does rests on two things. A verdict on a track or a via is
told by what is near it and is the same in whatever order that is looked at, so the numbers
that the entries have in a world where copper went out and came back do not matter. And the
walls round a pad's last spokes, which follow the routes, are a matter of what stands in the
spokes' rooms once anything has been placed after a route was taken out; the world is copied
only when that is so. It is not done in a run with a budget of nodes, where the nodes that
the searches before it took are what cuts a search, and such a run is on one thread.

*With an hour for a budget*, which no run comes near, so that nothing is decided by the
clock (eight boards, the default order, the load 3 to 6): 0.0.39 on one thread, 0.0.39 on
three and this release on three have the same connections open, the same vias, the same
track to a tenth of a millimetre and the same count of nodes on every one of them
(`nrfmicro` none open, 41 vias, 447.3 mm and 9.9 million nodes; `buspirate5-rev10` none, 284,
2881.9 mm, 13.2 million; `sweep-v2.2` 3, 23, 2104.1 mm, 25.9 million; `tinytapeout-demo` none,
279, 4880.2 mm, 40.1 million; `cynthion` 8, 428, 2998.1 mm, 20.5 million; `buspirate-rs232`,
`buspirate-flash-sop` and `lumenpnp-ringlight` as small). The runs on three threads take
49.4 s for 51.9 s on `tinytapeout-demo`, 34.7 for 35.8 on `cynthion`, 15.8 for 17.3 on
`buspirate5-rev10`, 35.5 for 35.8 on `sweep-v2.2` and 6.9 for 6.8 on `nrfmicro`. A test has
six nets of which three lose two vias each, on one, two, three and eight threads, and the
copper and the count of nodes of each.

### The nets of a round, the longest first

The nets of a round of this crate's own rounds are routed eight at a time against the same
claims, and the eight were begun in their order: a thread that takes the last of them is
alone with it while the others wait, if it is the longest. They are begun by the nodes that
each took the last time, the most first, and by their length before they have been routed.
None sees another's route, so the order is nothing to what each is routed as.
`corne-cherry` at 150 million nodes on three threads, the same 201 eights and the same
board: 5.79 s for 6.22 s of the eights' searches on the clock (13.5 s of them by the
threads' own time), and 12.16 s for 12.61 s for the rounds. What the rounds have beside the
eights is the turns after them, 2.8 s, of which 2.1 s are the 160 nets routed again at their
turn, and 3.5 s between the rounds.

### At budgets of nodes: the same runs

One thread, the crate alone behind cadlab's adapter, six runs at a time, the release's source
and 0.0.39 from crates.io one after the other (the load 3 to 6). At 40 million nodes, nine
boards in three orders, and at 150 million, eight boards in two: **each of the 43 runs has
the connections open by the run's count and by cadlab's DRC, the vias, the track and the
work of 0.0.39's.** 570 open at 40 million and 42 at 150 million on seven boards, 56 and 75
on `glasgow-revD1`, as the two releases before. Nodes a second over the whole run, 0.0.39
and this, the orders of a board together, at 40 million: `nrfmicro` 2.08 and 2.06 million,
`sweep-v2.2` 0.67 and 0.66, `corne-cherry` 1.27 and 1.26, `tinytapeout-demo` 0.66 and 0.65,
`buspirate5-rev10` 0.64 and 0.63, `lumenpnp-mobo` 0.61 and 0.60, `cynthion` 0.39 and 0.37,
`glasgow-revC3` 0.75 and 0.74, `glasgow-revD1` 0.83 and 0.79; at 150 million `corne-cherry`
1.75 and 1.73, `tinytapeout-demo` 0.74 and 0.74, `cynthion` 0.41 and 0.41, `glasgow-revC3`
0.55 and 0.54, `glasgow-revD1` 0.64 and 0.63. The same: on one thread with a budget of nodes
the release is the code that 0.0.39 is, and the sets were not measured in the same minutes.

### At 60 s

Both releases through cadlab at 60 s on three threads, side by side in the same minutes, two
runs of each at a time (the load 4.3 to 7.7): `corne-cherry`, `cynthion`, `buspirate5-rev10`
and `glasgow-revD1` in four orders and `tinytapeout-demo` in eight.

- **Where a run ends on its own, it is the same board sooner.** `buspirate5-rev10`, the four
  orders the same board to the via and the tenth of a millimetre: 16.5, 15.3, 14.7 and 15.2 s
  for 17.9, 16.8, 16.0 and 16.4 s. `cynthion`, the same four boards: 35.3, 36.8, 39.9 and
  34.9 s for 37.1, 38.6, 42.0 and 35.8 s. `corne-cherry`, three orders the same boards: 28.2,
  44.9 and 42.8 s for 30.0, 46.5 and 44.3 s; its default order's first run is ended by its
  deadline with both, 1 open in 53.5 s with 419 vias and in 53.7 s with 424.
- *`tinytapeout-demo`*: three orders the same boards, in 54.9, 47.6 and 41.6 s for 55.2, 51.6
  and 44.4 s. A run was out of its budget in one order of eight for three with 0.0.39, and
  the eight have 1, 0, 0, 0, 0, 0, 0 and 0 connections open for 1, 0, 3, 0, 0, 0, 1 and 1;
  the vias are 282 in the median with both, 279.1 and 281.6 in the mean.
- *`glasgow-revD1`*: the same four boards (12, 17, 7 and 12 open), in 56.4 to 59.0 s for 56.6
  to 58.5 s. Its first run is ended by its deadline before cleanup has begun, and has
  nothing of this.

By stage, this release in the same runs (the time on the clock, then the processor time):

| stage | `corne-cherry` | `tinytapeout-demo` | `glasgow-revD1` | `cynthion` |
|---|---|---|---|---|
| the board looked at, the escapes and the planes | 1.6 s; 1.6 s | 3.2 s; 3.4 s | 4.4 s; 5.0 s | 5.5 s; 6.0 s |
| routing: the rounds, and the passes that take nets out | 22.4 s; 34.0 s | 34.1 s; 34.6 s | 39.4 s; 53.4 s | 4.1 s; 4.7 s |
| settling what is open | 8.7 s; 9.2 s | 0.0 s; 0.0 s | 0.0 s; 0.0 s | 1.1 s; 1.1 s |
| cleanup | 7.0 s; 8.9 s | 7.7 s; 16.5 s | 0.4 s; 0.9 s | 14.4 s; 22.8 s |
| cadlab between the runs: its check, its fill, the plan of the next | 0.9 s; 1.5 s | 1.0 s; 2.1 s | 1.8 s; 3.9 s | 3.4 s; 8.0 s |
| cadlab after the last run: what the fill makes needless, the spokes, the ends in the pads | 2.2 s; 3.9 s | 3.0 s; 6.1 s | 8.5 s; 16.1 s | 5.4 s; 13.3 s |
| the whole run | 43.9 s; 61.4 s | 49.3 s; 64.3 s | 56.5 s; 83.1 s | 36.0 s; 61.8 s |

Cleanup has 1.1 s less on `corne-cherry`, 3.0 s on `tinytapeout-demo` and 1.4 s on `cynthion`,
for 0.2, 4.4 and 3.7 s more of the processor: the searches made ahead that a net's other
copper left unused, and the threads' own copies of the world.

### Beside cadlab's router at 60 s

The release's source behind cadlab's adapter, cadlab's own router and, on six boards, 0.0.39,
side by side in the same minutes: the twelve boards in four orders and `tinytapeout-demo` in
eight, four runs at a time on three threads each, the load 1.6 at the start and 4.1 to 7.6
from the first minute on. Open is cadlab's DRC count of unrouted on the board that left, for
both; open and the pads short of two necks by order; medians of the rest; the fastest and
the slowest run:

| board | open | vias | track | track outside pads | short of two necks | time |
|---|---|---|---|---|---|---|
| `buspirate-flash-sop` | equal (0/0/0/0 against 0/0/0/0) | equal (+0 %) 4 against 4 | better (-4 %) 442 against 461 | equal (-0 %) 431 against 431 | equal (0/0/0/0 against 0/0/0/0) | equal (1 to 1 s against 0 to 0 s) |
| `buspirate-rs232` | equal (0/0/0/0 against 0/0/0/0) | better (-20 %) 8 against 10 | equal (+1 %) 284 against 281 | equal (+1 %) 275 against 272 | equal (0/0/0/0 against 0/0/0/0) | equal (0 to 0 s against 0 to 0 s) |
| `lumenpnp-ringlight` | equal (0/0/0/0 against 0/0/0/0) | equal (+0 %) 3 against 3 | better (-6 %) 166 against 177 | equal (-1 %) 158 against 159 | equal (0/0/0/0 against 0/0/0/0) | equal (0 to 1 s against 0 to 0 s) |
| `nrfmicro` | equal (0/0/0/0 against 0/0/0/0) | better (-14 %) 41 against 47.5 | better (-3 %) 437 against 450 | equal (+2 %) 421 against 414 | equal (0/0/0/0 against 0/0/0/0) | equal (2 to 14 s against 2 to 12 s) |
| `sweep-v2.2` | equal (3/3/3/3 against 3/3/3/3) | better (-18 %) 21 against 25.5 | better (-2 %) 2115 against 2159 | equal (-1 %) 2104 against 2122 | better (0/1/0/1 against 2/1/2/1) | better (36 to 37 s against 52 to 53 s) |
| `corne-cherry` | better (1/0/1/1 against 1/3/2/4) | better (-7 %) 425.5 against 459 | better (-8 %) 7492 against 8103 | better (-6 %) 7421 against 7915 | better (2/2/1/1 against 3/3/2/2) | worse (28 to 54 s against 21 to 32 s) |
| `tinytapeout-demo` | better (0/0/0/0/0/0/1/0 against 19/1/4/5/1/2/2/9) | worse (+2 %) 279.5 against 273 | equal (-1 %) 4852 against 4894 | worse (+3 %) 4797 against 4672 | better (1/2/0/0/1/1/4/3 against 20/12/10/16/16/14/17/19) | equal (40 to 54 s against 34 to 61 s) |
| `buspirate5-rev10` | better (0/0/0/0 against 4/2/4/4) | better (-6 %) 285 against 302 | better (-8 %) 2877 against 3129 | better (-4 %) 2793 against 2911 | better (10/12/11/14 against 22/24/30/21) | better (14 to 16 s against 20 to 33 s) |
| `lumenpnp-mobo` | better (0/0/0/0 against 9/23/10/14) | better (-2 %) 331 against 339 | better (-3 %) 5291 against 5446 | equal (+1 %) 5214 against 5160 | better (0/0/0/0 against 2/4/2/1) | better (30 to 34 s against 60 to 62 s) |
| `cynthion` | better (8/8/8/8 against 28/29/30/28) | better (-13 %) 417 against 478 | better (-5 %) 3034 against 3184 | equal (-1 %) 2961 against 2981 | equal (1/1/1/1 against 1/1/1/1) | better (34 to 39 s against 62 to 62 s) |
| `glasgow-revC3` | better (9/6/9/8 against 35/29/39/30) | better (-3 %) 403 against 415 | worse (+5 %) 3633 against 3448 | worse (+11 %) 3547 against 3190 | equal (0/0/0/0 against 0/0/0/0) | equal (59 to 59 s against 60 to 62 s) |
| `glasgow-revD1` | better (12/17/7/12 against 127/109/114/144) | worse (+8 %) 1145.5 against 1061.5 | worse (+18 %) 10576 against 8953 | worse (+22 %) 10323 against 8432 | better (8/23/10/22 against 43/31/41/47) | better (56 to 58 s against 60 to 63 s) |

Of 72 cells 36 are better, 28 equal and 8 worse, where 0.0.39 had 34, 28, one overlapping and
9; with 0.0.39's own runs of these minutes on the six boards that have them the count is the
same 36, 28 and 8. The two cells that moved did not move by this release:
`tinytapeout-demo`'s connections open, which either release has at none in seven orders or
at one or three in an order more, and `lumenpnp-mobo`'s track and track outside the pads,
where this crate's four runs are the boards they were and cadlab's router's are others. The
worse cells are 0.0.39's but for `lumenpnp-mobo`'s track outside the pads: `corne-cherry`'s time, `tinytapeout-demo`'s vias and
track outside the pads, `glasgow-revC3`'s track and track outside the pads, `glasgow-revD1`'s
vias, track and track outside the pads. `tinytapeout-demo` is out of its budget in none of
these eight orders for three with 0.0.39, and in two of the eight more of the census.

By the nets that both routers complete:

| board | nets both complete (median of the pairs) | vias on them | track on them | track outside pads on them | nets only guide completes, their track and vias; only native |
|---|---|---|---|---|---|
| `buspirate-flash-sop` | 12 | equal (+0 %) 4 against 4 | better (-4 %) 442 against 461 | equal (-0 %) 431 against 431 | 0 nets, 0 mm, 0 vias; 0 |
| `buspirate-rs232` | 30 | better (-20 %) 8 against 10 | equal (+1 %) 284 against 281 | equal (+1 %) 275 against 272 | 0 nets, 0 mm, 0 vias; 0 |
| `lumenpnp-ringlight` | 12 | equal (+0 %) 3 against 3 | better (-7 %) 166 against 177 | equal (-1 %) 158 against 159 | 0 nets, 0 mm, 0 vias; 0 |
| `nrfmicro` | 41 | better (-14 %) 41 against 47.5 | better (-3 %) 437 against 450 | equal (+2 %) 421 against 414 | 0 nets, 0 mm, 0 vias; 0 |
| `sweep-v2.2` | 40 | better (-21 %) 19 against 24 | equal (-1 %) 2034 against 2060 | equal (-0 %) 2024 against 2028 | 0 nets, 0 mm, 0 vias; 0 |
| `corne-cherry` | 149 | better (-9 %) 397.5 against 434.5 | better (-7 %) 7202 against 7759 | better (-6 %) 7139 against 7593 | 2.5 nets, 164 mm, 12.5 vias; 1 |
| `tinytapeout-demo` | 135.5 | worse (+6 %) 246.5 against 233 | equal (+1 %) 4740 against 4680 | worse (+4 %) 4689 against 4496 | 1 nets, 60 mm, 25.5 vias; 0 |
| `buspirate5-rev10` | 181 | equal (+1 %) 252.5 against 250 | better (-3 %) 2848 against 2927 | equal (+2 %) 2767 against 2720 | 2 nets, 30 mm, 33 vias; 0 |
| `lumenpnp-mobo` | 187.5 | worse (+5 %) 258 against 245 | better (-3 %) 4928 against 5057 | equal (+1 %) 4864 against 4810 | 5.5 nets, 312 mm, 70 vias; 4 |
| `cynthion` | 310.5 | better (-23 %) 277 against 360.5 | better (-6 %) 2695 against 2870 | better (-3 %) 2634 against 2714 | 15.5 nets, 345 mm, 134.5 vias; 0 |
| `glasgow-revC3` | 199 | better (-13 %) 219.5 against 251 | equal (+2 %) 2668 against 2627 | worse (+6 %) 2615 against 2461 | 19 nets, 941 mm, 185.5 vias; 7 |
| `glasgow-revD1` | 499.5 | better (-3 %) 766 against 788 | worse (+8 %) 8126 against 7548 | worse (+11 %) 7933 against 7150 | 49.5 nets, 2392 mm, 298 vias; 10 |

16 better, 14 equal and 6 worse of 36, for 16, 15 and 5: `lumenpnp-mobo`'s vias on those nets
are +5 % for +2 %, with this crate's boards the ones they were and cadlab's router leaving
9, 23, 10 and 14 connections open for 9, 10, 6 and 14, so that the nets both complete are
others.

In the 60 runs of the twelve boards in four orders and `tinytapeout-demo` in sixteen: no pad
of a pour without a spoke, no DRC error other than unrouted, no solder pad made solid by a
track, and no run whose count of open connections is not cadlab's DRC's (cadlab's router's
is another in 16 runs of 52, by what it counts).

### What a shorter budget returns on `corne-cherry`

Its time is still outside cadlab's router's, 28 to 54 s against 21 to 32 s. The board at a
budget of 20, 30, 40 and 60 s, four orders, cadlab's router at 60 s in the same minutes (the
load 1.8 to 5.0; open and the pads short of two necks by order, medians of the rest):

| | open | vias | track | outside pads | short of two necks | s |
|---|---|---|---|---|---|---|
| this crate, 20 s | 18/48/48/50 | 405 | 6812 | 6751 | 0/1/1/1 | 16 to 19 |
| 30 s | 6/0/3/3 | 452 | 7547 | 7479 | 2/2/1/2 | 25 to 26 |
| 40 s | 1/0/4/1 | 439 | 7510 | 7441 | 1/2/1/1 | 27 to 36 |
| 60 s | 1/0/1/1 | 425.5 | 7508 | 7438 | 2/2/1/1 | 28 to 56 |
| cadlab's router, 60 s | 1/3/2/4 | 459 | 8103 | 7915 | 3/3/2/2 | 21 to 32 |

At 40 s the third order has 4 connections open for 1 at 60 s: the minute still buys
connections, and no end is made sooner.

### What the host's fills are

cadlab fills its zones itself and gives the crate its fill when it asks. Counted in a run at
40 million nodes on one thread (the fills are on a thread a layer whatever the run has):

| board | fills in the run | asked by the crate; by the check of a run; by the steps after the last run | zone layers a fill | of them kept from the fill before | time of the fills, of the run |
|---|---|---|---|---|---|
| `cynthion` | 14 | 3; 4; 7 | 61 | 25 % | 20.7 s of 51.4 s |
| `tinytapeout-demo` | 11 | 5; 3; 3 | 7 | 14 % | 7.5 s of 58.9 s |
| `glasgow-revD1` | 5 | 0; 1; 4 | 14 | 36 % | 7.1 s of 48.7 s |
| `corne-cherry` | 5 | 1; 1; 3 | 1480 (teardrops) | 25 % | 1.9 s of 30.6 s |

Each is of another board: no fill is made twice (cadlab keeps the last four whole and the
last of each zone layer, and a board asked again costs a comparison). A zone layer is made
again when anything it is made from differs, and a pour over the whole board differs with
any track. Of a zone layer made again on `cynthion`, the opening that takes out what is
narrower than the least width is 42 %, the area that other nets' copper leaves 37 %, the
spokes 9 %. Those are `polyclip`'s operations on the whole of a pour, and nothing of them
was changed here.

### Tried and not kept, and what remains open

- *The four changes to a node of the passes*, above: no time that a run shows.
- *The passes that take nets out and settle the open connections on more than one thread*:
  not built. Each search there is of the world that the search before it left, and most of
  them change it; what is made ahead would be made again.
- *A fill of the part of a pour that changed*: not built. It would have to give the polygons
  that the whole fill gives, to the nanometre, and `polyclip` rounds a vertex where the
  whole is cut.
- *`corne-cherry`'s time through cadlab*, 28 to 54 s where cadlab's router takes 20 to 32 s:
  its routing is 22 s of which the rounds are on 1.5 threads, what settles its open
  connections 9 s on one, and the run for shorter ways that cadlab asks for after the last
  run 5.5 s in every order (4 s of cleanup on one thread and 1.5 s of cadlab's check), for
  7 mm of track less and a via more in one order and 39 mm more with 14 vias fewer in
  another. Whether that run's searches are of the kind that can be made ahead, few of them
  ending with another route, was not measured.
- *`glasgow-revD1`'s minute* ends at 54 to 59 s with a first run that its deadline ends at
  42 s: what cadlab keeps for its steps after the last run is more than they take.

## PCB routing with the searches of this crate's rounds made faster

0.0.38 left seven cells worse beside cadlab's own router at 60 s, and each of them is at
bottom a matter of time: `corne-cherry`'s rounds, the cleanup that `glasgow-revD1`'s first
run has no time for, the deadline that ends `tinytapeout-demo`'s first run in three to five
orders of eight. This round looked at where the time goes with a profiler, and changed
nothing that a run decides. **Whether an end of a connection is walled in is asked from the
nodes nearest a source first, which is the same answer in a hundredth of the nodes; the
heap of a search of this crate's own rounds has four items under each and compares a rank
and its node as one number. At budgets of nodes every one of 43 runs has the connections
open, the vias, the track and the work that 0.0.38 has; the rounds themselves take 0.79 of
the time on `corne-cherry` and 0.73 on `glasgow-revD1` on one thread.**

### Where the time went

`perf` is not on the machine that measured this and its kernel does not let a process be
sampled from outside it, so the profile is the process's own: a timer signal 499 times a
second of its processor time, and the stack at each (the `pprof` crate behind a hook in
cadlab's `route_bench`, in no commit). Lines within a function it does not tell; for those,
`callgrind` counted the instructions of `whole::Board::search` alone over the first four
rounds of `corne-cherry`. Through cadlab at 60 s on three threads, 0.0.38, the default order;
shares of the processor time of the whole process (cadlab's fills and checks are in it):

| board | processor time of the minute | this crate's own rounds | the rounds of cadlab's router | the passes (`detail`) | the rest |
|---|---|---|---|---|---|
| `corne-cherry` | 37 s of 54 s | 27 %: searches 20 %, walled-in ends 7.5 % | | 32 %: rip-up 22 %, settling 10 % | cadlab's DRC 7.6 % |
| `glasgow-revD1` | 38.5 s of 58 s | 43 %: walled-in ends 31 %, searches 10 % | | 11 % | the fills' polygons 14 % |
| `tinytapeout-demo` | 32 s of 52 s | 3.5 % (each net routed again) | 52 %: searches 20 % of the whole, claims 9 % | 26 %: the search for fewer vias 16 % | the fills' polygons 6.5 % |
| `cynthion` | 25 s of 38 s | 5 % | | 27 %: the search for fewer vias 18 % | the fills' polygons 18 %, points in rings 6.7 % |

A minute on three threads has 180 s of processor time and the runs use 25 to 39 s of it:
the rounds route eight nets at a time and wait for the longest search of the eight, and
everything after them is one thread.

- **Walled-in ends** (`whole::Board::pocket`). Before a connection is first searched for, the
  nodes that reach its targets over what the board's own copper allows are looked at, 20 000
  at most, and a connection with no source among them is not searched for. They were looked
  at in the order of a stack, which wanders: with a way to a source, the 20 000 were most
  often used up before a source was met, in memory that no cache holds: 2577 calls in
  `glasgow-revD1`'s first round, 31 % of the processor time of its minute on three threads
  and 3 s of its rounds' 45 s on one. What is answered does not depend on the
  order (yes where what reaches the targets is 20 000 nodes at most with no source among
  them), so the nodes nearest the source nearest the first target are looked at first: 0.14 s
  for the same 2577 answers.
- **The heap of a search** (`whole::Ranked`). A binary heap of a rank of 64 bits and a node,
  compared one after the other. `callgrind` put a quarter of a search's instructions in its
  pops after the heap had four items under each, and more before; half of what is popped in
  the first rounds is an entry that a cheaper way has left behind (115 million pops for 75
  million nodes on `corne-cherry`). Now four items under each, and the rank and the node one
  number of 128 bits. The order that nodes come out in is the order of that number, as it
  was.
- **What a search knows of a node in one place** (`whole::Slot`), a node's layer and cell by
  divisions of 32 bits, and the cells too near a via of the net's own wires marked in a map
  when the via is laid in place of every such via asked at every node (the squared distance
  in cells against the largest whole number whose root times the pitch is less than the
  distance asked for, which is the same question): none of the three is told from noise in
  the time of the rounds. They are in the release as they leave less to do.

The rounds alone, on one thread at a budget of nodes (the same rounds to the node: 33 rounds
and 75.0 million nodes on `corne-cherry` at 150 million, 40 rounds and 124.6 million on
`glasgow-revD1` at 600 million; two runs side by side, the load 2 to 5):

| step | `corne-cherry`: s; million nodes a second | `glasgow-revD1` |
|---|---|---|
| 0.0.38 | 19.1 to 20.1; 3.7 to 3.9 | 45.3; 2.75 |
| walled-in ends from the nearest node | 19.6 | 42.1 to 43.6; 2.9 |
| four items under each | 18.9 to 19.0 | 40.9 to 41.1 |
| a node in one place | 19.0 | 41.1 |
| the net's own vias in whole numbers | 17.9 to 18.8 | 38.1 |
| rank and node one number | 15.1; 5.0 | 33.4; 3.7 |
| the net's own vias marked, 32 bits | 15.3; 4.9 | 32.8; 3.8 |

1.27 times the nodes a second on `corne-cherry`'s rounds and 1.38 times on
`glasgow-revD1`'s. The target of the round was 1.5 times on the whole run, and that is not
reached: over a whole run at a budget of nodes the gain is 1 to 8 % (below), since such a
run gives the rounds up on most boards and is the passes, whose search was not changed.

### At budgets of nodes: the same runs

Nine boards in three orders at 40 million nodes and eight boards in two at 150 million, one
thread, this release beside 0.0.38, ten runs at a time: **each of the 43 runs has the
connections open by the run's count and by cadlab's DRC, the vias, the track to the tenth of
a millimetre and the work of 0.0.38's.** 570 open at 40 million (`nrfmicro` none;
`sweep-v2.2` 3, 3 and 3; `corne-cherry` 36, 28 and 22; `tinytapeout-demo` none, none and 1;
`buspirate5-rev10` and `lumenpnp-mobo` none; `cynthion` 8, 8 and 8; `glasgow-revC3` 12, 15
and 12; `glasgow-revD1` 145, 136 and 130); 42 at 150 million on seven boards and 56 and 75
on `glasgow-revD1`. Nodes a second over the whole run, by board, the three or two orders
together:

| board | 40 million: 0.0.38; this release | 150 million |
|---|---|---|
| `nrfmicro` | 1.89; 2.01 million a second | |
| `sweep-v2.2` | 0.64; 0.64 | 0.67; 0.64 |
| `corne-cherry` | 1.17; 1.22 | 1.58; 1.71 |
| `tinytapeout-demo` | 0.65; 0.66 | 0.69; 0.72 |
| `buspirate5-rev10` | 0.62; 0.63 | 0.62; 0.64 |
| `lumenpnp-mobo` | 0.58; 0.60 | 0.57; 0.60 |
| `cynthion` | 0.36; 0.37 | 0.38; 0.40 |
| `glasgow-revC3` | 0.71; 0.74 | 0.53; 0.54 |
| `glasgow-revD1` | 0.76; 0.79 | 0.62; 0.63 |

### At 60 s

Through cadlab on three threads, 0.0.38 and this release side by side in four orders, four
runs at a time (the load 2.5 to 5.6):

| board, four orders | 0.0.38 | this release |
|---|---|---|
| `corne-cherry` | 4, 0, 1, 1 open; 427, 450, 432, 419 vias; 53.4, 35.3, 52.6 and 50.9 s | 1, 0, 1, 1; 425, 450, 432, 419; 54.2, 30.6, 47.9 and 44.9 s |
| `glasgow-revD1` | 16, 22, 7, 8 open; 1181, 1187, 1183, 1210 vias; 10 503, 10 138, 10 950, 10 662 mm | 12, 17, 7, 12; 1132, 1138, 1183, 1153; 10 584, 10 437, 10 950, 10 568 mm |
| `tinytapeout-demo` | 0, 0, 3, 0 open; 303, 275, 306, 290 vias; its first run ended by its deadline in two orders | 0, 0, 1, 0; 282, 282, 271, 282; in none |

`corne-cherry`'s three orders whose first run ends on its own end with the board they had,
4.7 to 6.0 s sooner; its default order's first run is ended by its deadline with either
build, and is another board. `glasgow-revD1`'s rounds are ended by their share of the
time, not by their number, so that what is saved goes to more rounds and not to the
passes (with the walled-in ends alone changed, 39 rounds for 32 in the default order and
119 million nodes for 112 as they end): 4 % fewer vias in three orders of four and the
same board in the fourth. In the 60 runs of the
twelve boards with this release (`tinytapeout-demo` in sixteen orders, the load 1.4 to
4.9) no pad of a pour is without a spoke, cadlab's DRC has no error but the connections
left open, and the count that the run reports is the DRC's in every one.

### Beside cadlab's router, and two things measured that change nothing

The round had two more items, and both were measured before anything was built for them.

**Where the routes enter their pads.** 0.0.38 has more track outside the pads of its nets than
cadlab's router on the large boards, on the nets both complete, and the thought was that a
route aimed at a pad's centre goes further round the pad than one that ends at the first
copper of it. By pad of those nets, the net's track inside the pad's outline, within 2 mm of
the pad's box and outside the pad, and farther (both routers at 60 s in the same minutes,
two orders):

| board, order | inside the pads: this crate; cadlab's router | within 2 mm, outside the pad | farther |
|---|---|---|---|
| `glasgow-revC3`, 0 | 58; 171 mm | 1420; 1415 mm | 1178; 1071 mm |
| `glasgow-revC3`, 2 | 56; 164 | 1400; 1387 | 1219; 1045 |
| `tinytapeout-demo`, 0 | 53; 185 | 1137; 1223 | 3568; 3340 |
| `tinytapeout-demo`, 2 | 52; 187 | 1134; 1212 | 3661; 3246 |

Near its pads this crate has as much track as cadlab's router or 6 to 7 % less, and its
tracks cross the pads' outlines on the same sides (`glasgow-revC3`: 37 % a long side, 52 % a short one, 11 %
near a corner, for 35, 53 and 12 %). What it has more of is in the open. Routing only the
nets that cadlab's router completes, its track on them is -1.4 %, -1.8 %, +3.0 % and +3.9 %
beside cadlab's on `glasgow-revD1` in four orders, where with the whole board it is +6.5 to
+8.4 %: a part of the track in the open is what the nets cost that only this crate
completes, 49 of them with 2392 mm on that board. Nothing was changed.

**What a shorter budget returns.** `corne-cherry` takes 29 to 54 s of its minute where
cadlab's router takes 20 to 32 s. The board returned at a budget of 20, 30, 40 and 60 s, four
orders, connections open by cadlab's DRC: 42, 47, 48 and 50; 9, 0, 3 and 6; 13, 0, 1 and 1;
1, 0, 1 and 1, where cadlab's router has 1, 3, 2 and 4 at 60 s. The time buys the
connections, and no rule ends a run sooner.

**The table.** Twelve boards in four orders (`tinytapeout-demo` in eight), this release and
cadlab's router side by side at 60 s on three threads, the load 3.6 to 7.7, judged cell by
cell as in the sections below: of 72 cells 34 better, 28 equal, 1 overlapping and 9 worse,
for 35, 29, 1 and 7 with 0.0.38 in its own minutes and 35, 29, none and 8 with 0.0.38 in
these. The cells that differ are cadlab's router's own runs on `lumenpnp-mobo` (this
crate's four runs are the boards they were), `tinytapeout-demo`'s third order, which ends
with none or with three connections open with either release, and `tinytapeout-demo`'s
vias, 279.5 against 273 where 0.0.38 has 274: over three sets of eight orders side by side
the medians are 279.5, 281.5 and 279.5 with this release and 274, 282 and 275 with 0.0.38,
and the means of the 24 runs 280.1 and 279.1. A faster search changes which run a deadline
ends; nothing says that it changes more.

### Tried and not kept, and what remains open

- **The time of the passes was not changed.** They are the search of `detail` over
  `search::Search`, which is this crate's for every domain: on `glasgow-revD1` at 40
  million nodes 56 % of the processor time, of which the search's own loop 27 %, the
  steps of a node 20 %, the spatial index 14 %, the number of a node 10 % and a step's
  price 8 %. Nothing in it is a single place as the walled-in ends were.
- **The rounds wait for their longest search**: eight nets at a time, 32 s of searches in
  the 20 s of `corne-cherry`'s rounds on three threads. A round late in a negotiation
  has fewer than eight nets to route and one long search among them; the batches were not
  changed, since what a net is routed as depends on them.
- **The fills** are `polyclip`'s, 6 to 18 % of the processor time on the boards with pours
  on several layers, and were not looked at.

## PCB routing with a negotiation that ended with one point shared not begun again

0.0.37 left nine cells worse beside cadlab's own router at 60 s, and this round took three of
its open items in order: the rounds of cadlab's router on two layers (`Rounds::Native`, the
sections below), for the time of `nrfmicro` and `corne-cherry`; the track of the two
`glasgow` boards; `tinytapeout-demo`'s vias. What was measured says where each comes from,
and one thing is changed in this crate. **A negotiation for every net that ended with one
point still shared is no longer begun again on other grids and in other orders: `nrfmicro`
has two orders of eight that spent 7 to 8 s of 22 s in nine such tries and ended with the
board that they end with without them, to the via and the tenth of a millimetre, in 15 s.
The rounds on two layers are not a gain, the pass for the long ways round that was built
for `glasgow-revD1` gains no more than cleanup does in the same time, and a pass for fewer
vias repeated to its end saves 1.3 % of `tinytapeout-demo`'s vias for 4 to 7 s; none of
the three is in the release.**

### A negotiation that ended with one point shared

At the pitch, a negotiation that ends on its own with points still shared is begun again on
grids of two thirds, a half and a third of the pitch and in other orders of the nets, nine
tries, and the one with the fewest shared is kept (0.0.23). One point shared is one wire to
take out, which the passes lay again; the tries after such a negotiation go on for one that
ends with none. `nrfmicro` through cadlab at 60 s on three threads, eight orders, the same
build with the rule off and on (the load 3.4 to 4.8):

| order | off: s; vias; mm | on | the negotiation kept: rounds, shared, wires out |
|---|---|---|---|
| 0 | 6.9; 48; 431.0 | 7.4; 41; 447.3 | 23, 0, 0 off; 32, 1, 1 on |
| 2 | 2.6; 40; 427.3 | 2.6; 40; 427.3 | 19, 0, 0 |
| 3 | 5.6; 44; 418.3 | 5.7; 44; 418.3 | 37, 0, 0 |
| 4 | 22.0; 41; 455.0 | 14.8; 41; 455.0 | 42, 1, 1 |
| 5 | 6.5; 37; 439.3 | 6.5; 37; 439.3 | 17, 0, 0 |
| 6 | 58.4; 49; 422.7, 2 open | 58.5; 49; 422.7, 2 open | 100, 30, 3 |
| 7 | 23.1; 47; 440.9 | 15.0; 47; 440.9 | 29, 1, 1 |
| 8 | 14.0; 44; 437.7 | 13.3; 44; 437.7 | 45, 1, 1 |

Orders 4 and 7 end 7 to 8 s sooner with the board they had. Order 0 changes: its negotiation
with one point shared is kept where a later try ended with none, and the passes lay the wire
again with 76 nets taken out, for 7 vias fewer and 16 mm more. With two points allowed the
eighth order changes too, 60 vias for 44, and that is not what is released. What is left of
order 4 is one thread settling the last connections, 10.5 s of the 14.8 s with 233 nets
taken out, which the tries were not.

At a budget of nodes (one thread, guide alone through cadlab; nine boards in three orders at
40 million, eight in two at 150 million) every one of the 43 runs has the connections open
and the vias that it had: 570 open at 40 million, 42 at 150 million on D131's seven boards
and 56 and 75 on `glasgow-revD1`. `nrfmicro`'s default order takes 7.8 s for 10.9 s.

### The rounds of cadlab's router on two layers

With the rounds asked for on two layers, at 150 million nodes in four orders beside this
crate's own (open, vias, seconds on one thread):

| board | this crate's rounds | the rounds of cadlab's router |
|---|---|---|
| `corne-cherry` | 8, 0, 0, 6; 439, 448, 453, 425; 118, 43, 59, 110 s | 3, 1, 0, 3; 477, 499, 484, 460; 76, 81, 40, 96 s |
| `nrfmicro` | 0, 0, 0, 0; 48, 40, 44, 41; 9, 3, 8, 25 s | 1, 1, 0, 0; 39, 57, 44, 51; 40, 31, 15, 16 s |

- *`corne-cherry`, the default order.* The rounds end after 32, in 14.7 s on a thread, with
  543 vias, 8292 mm and nothing open; made legal 539 and one open; after their pass for
  fewer vias 485 and 8398 mm. The passes take 423 of the 482 connections as they are handed
  and make the rest with 171 nets taken out, 491 vias and 8445 mm before cleanup and 480
  and 8442 mm after it; the board has 477, 8140 mm and 3 open. This crate's own rounds in
  another order of the same set: 99 rounds, 433 connections taken, 463 vias and 8207 mm
  before cleanup, 453 and 8096 mm after, 448 and 7463 mm on the board.
- *The fills that are no planes.* `corne-cherry` has 925 zones, and 179 fills of 115 nets
  are no planes: the teardrops of its pads, 1 to 1.5 mm across. They are copper of their
  nets to every other net in the rounds, as they are to the passes, where cadlab's router
  fills them again around the routes; no other board of two layers in the corpus has one,
  and `nrfmicro` has no zone. With them as fills alone in the rounds: 40 rounds, 564 vias
  and 8565 mm at their end with 32 points shared, 483 and 8494 mm after the pass for fewer
  vias, 406 connections taken by the passes, and the board with 491 vias, 8196 mm and 2
  open, for 477, 8140 mm and 3. So they are not what the rounds lose by on this board.
- *`nrfmicro`.* The rounds end in 0.4 to 0.5 s after 24 to 30 with 50 or 51 points still
  shared, the grid being the pitch and the passages between its parts narrower: 4 or 5
  wires go, and the passes take 5 to 11 s and 173 to 320 nets taken out to make them, with
  one connection left open in two orders of four. This crate's own rounds begin again on
  finer grids there (0.0.23).

Not adopted, and cadlab's adapter asks for the rounds on more than two layers as it did.

### The long ways round on glasgow-revD1

On the nets that both routers complete, `glasgow-revD1` has 7 % more track than with cadlab's
router and 11 % more outside the pads (cadlab's D133). Where it comes from, measured:

- *Its first run has next to no cleanup.* Routing begins 3.8 s into the run and its forty
  rounds at twice the pitch end 28 s after that; the passes have what is left of the first
  run's 42 s, 8 to 10 s, of which by the run's shares rip-up has three quarters and cleanup
  a third of the rest, 0.6 to 0.8 s (the shares, not a measurement: the run tells no phase
  between routing and its report at 42.05 s). No net is routed again among the others (that
  asks for 2 s), and cleanup's searches take out nothing in one run and 212 and 222 mm of
  11 700 in two others before the part ends.
- *The rip-up is what the track is paid in.* With routing given three tenths of what is
  left, for three quarters, and cleanup nine tenths of the rest, the board has 34 connections open for 9 in the default order and
  34 for 31 in another, 1085 and 1083 vias for 1200 and 1179, and 9652 and 9628 mm for
  10 692 and 9766 mm: before cleanup 10 870 mm with 140 nets taken out for 11 744 mm with 306.
  On the nets both complete that board has 2.0 % more track than cadlab's router's and 8 %
  fewer vias, where the board as released has 9 % more and 1 % more. cadlab's router leaves
  117 to 146 connections open there.
- *Each net routed again among the others, with the time*: 3.4 s for its first time through
  the nets, which is for fewer vias, 336 nets looked at, 29 kept, 46 vias fewer; 1.1 s for
  the second, 158 kept in all, 142 mm shorter.
- *A pass for the long ways round alone* was built: the nets more than half as long again as
  the tree over their pads, the one with the most to lose first, each taken out and searched
  for once among the others by the negotiation's search, kept if shorter, with a via more
  where that is 5 mm less. With all the time it wants, on the board as the rip-up leaves
  it: 50 nets looked at, 19 kept, 169 mm and 7 vias less in 1.05 s in one order; 35, 15 and
  46 mm in 0.74 s in another. In eight tenths of what cleanup has, under a second: one net, 45 mm,
  and 15 nets, 7 kept, 40 mm, and cleanup's own searches, which had that time, then take out
  nothing where they took 212 and 222 mm in the same two orders. So in the time there is it
  gains less than what it displaces, and with more time it gains what routing each net again
  does; the time is what is missing. Not kept.
- *More of the first run for cleanup* (routing 0.65 or 0.55 of what is left, cleanup 0.55 or
  0.6 of the rest), two orders: 16 and 13 connections open with 1177 and 1205 vias and
  10 409 and 10 693 mm, and 12 and 13 with 1191 and 1202 and 10 884 and 10 614 mm, where
  five runs of the release's shares in the same hour have 9 to 16 and 23 to 31 open, 1179
  to 1212 vias and 9766 to 10 692 mm: nothing to tell from what two runs of one build differ
  by.

`glasgow-revC3` is another case: each net is routed again among the others there, in 0.6 s
(372 looked at, 66 kept, 122 mm), and its track on the nets both complete is within 1 % of
cadlab's router's. Its track outside the pads is 6 to 7 % more on those nets, and that is
copper: cadlab's router has 163 to 207 mm of their track inside the pads of its nets and
this crate through cadlab 54 to 76 mm, its tracks being ended at the first copper of their
pads, so the totals are level where the routes outside the pads are longer.

### The pass for fewer vias, to its end

The pass for fewer vias of the rounds of cadlab's router goes through the nets once. Gone
through again until a time saves nothing, `tinytapeout-demo` at 150 million nodes in eight
orders: 2 to 17 vias fewer as the rounds' stage ends (two or three times through, 2.2 s on
a thread each), and on the board 256, 275, 285, 265, 274, 260, 266 and 260 vias for 260, 274,
297, 273, 269, 269, 267 and 260, 267.6 in the mean for 271.1. 1.3 % for 4 to 7 s that a run
of 60 s does not have, where the first run is ended by its deadline in three to five orders
of eight. Not kept.

Where `tinytapeout-demo`'s first run goes, in two orders at 60 s: routing begins 3 s in;
the rounds take 20 to 28 s, 16 to 31 of them; what makes their routes legal and the pass
for fewer vias 4 s; cleanup 10 to 12 s where it is not cut, of which each net routed again
among the others 1.8 to 2.0 s (300 looked at, 23 or 24 kept, 12 to 15 vias), the joining of
what the routes cut off the fills 2.0 to 3.2 s, and cleanup's own searches 6.6 to 7.8 s for
some 6 vias and 55 mm.

### At 60 s and at budgets of nodes, as released

Through cadlab at 60 s on three threads, four runs at a time, this release beside 0.0.37 and
cadlab's own router in the same minutes (the load 3.9 to 6.0; cadlab's adapter of this round
also ends a run that moves routes sooner, which is `glasgow-revC3`'s track here):

| board, four orders | 0.0.37 | this release | cadlab's router |
|---|---|---|---|
| `nrfmicro` | 48, 40, 44, 41 vias; 429 mm; 7, 2, 6 and 21 s | 41, 40, 44, 41; 437 mm; 7, 2, 6 and 15 s | 46, 46, 49, 51; 450 mm; 12, 2, 6 and 4 s |
| `sweep-v2.2` | 3 open in each; 21 vias; 2115 mm; 36 to 37 s | the same; 36 to 38 s | 3 in each; 25.5; 2159 mm; 52 to 54 s |
| `corne-cherry` | 1, 0, 1, 1 open; 431 vias; 7487 mm; 33 to 55 s | 5, 0, 1, 1; 437.5; 7477 mm; 34 to 54 s | 1, 3, 2, 4; 459; 8103 mm; 20 to 32 s |
| `glasgow-revC3` | 9, 6, 9, 8 open; 403.5 vias; 3647 mm | 9, 6, 9, 8; 403; 3632 mm | 35, 34, 39, 30; 419.5; 3448 mm |
| `glasgow-revD1` | 14, 28, 7, 8 open; 1179 vias; 10 694 mm | 16, 28, 7, 8; 1182; 10 583 mm | 146, 152, 123, 144; 1035; 8916 mm |
| `tinytapeout-demo`, eight | 2 open in all; 277.5 vias; 4859 mm | 2; 277.5; 4850 mm | 43; 273; 4894 mm |

`corne-cherry`'s default order has its first run ended by its deadline and is 1, 4 or 5
connections open from one run to the next with either build: measured twice more side by
side, 4, 0, 1, 1 and 1, 0, 1, 1 with this release and 1, 0, 1, 1 and 5, 0, 1, 0 with 0.0.37.
No pad of a pour is without a spoke in the 60 runs of the twelve boards (`tinytapeout-demo`
in sixteen orders), and the count of open connections that the run reports is the DRC's in
every one, as in the 43 runs at budgets of nodes.

At budgets of nodes, above: the 43 runs to the connection and the via as 0.0.37 has them.

### What remains open

- `nrfmicro`'s slowest order of the four is 15 s for 13 s with cadlab's router, one thread
  settling its last connections; `corne-cherry`'s time is where it was.
- `glasgow-revD1` has no time for cleanup in a minute, and what it has in track over
  cadlab's router on the nets both complete is the price of the connections that its rip-up
  makes and cadlab's router leaves open.
- `tinytapeout-demo`'s vias: cleanup's own searches are the slowest part of its first run
  for what they save, and were not made faster.

## PCB routing with the rounds of cadlab's router brought to their end

0.0.36 left one board worse with the rounds of cadlab's own router (`Rounds::Native`, the
sections below) than with this crate's own: `tinytapeout-demo`, with 4 % more vias, its first
run ending at its deadline in every order. This round measured why, and what it found is the
rounds, not what comes after them. **A point that stays shared now costs 2 more in each round
that it was, for 0.5, and the rounds reach their end in 119 million nodes for 167 in the mean
of eight orders; the pass for fewer vias after them has a time of its own where their share
ended them; and the maps that the look at the board makes are the rounds' if the board is the
one looked at. Through cadlab at 60 s, in sixteen orders side by side, `tinytapeout-demo` has
277 vias in the median for 283.5 and 7 connections open in all for 11; with the three of
this section and a first run of 0.78 of the minute for 0.7 in cadlab, 277 where 0.0.36 had
302.5 in other minutes of the round, this crate's own rounds 288 and cadlab's router 273.**

All of it measured through cadlab's adapter (its route_bench; at 60 s three threads a run and
four runs at a time, the load 2 to 7 on a machine of 64 threads; at a budget of nodes one
thread, where a run is the same every time). A run at 60 s ends by the clock, and two runs of
one order differ by as much as two orders do: the sets below are of eight or sixteen orders,
side by side in the same minutes.

### What a run that is cut loses

`tinytapeout-demo` in eight orders at 60 s with 0.0.36, by what the first run says of itself:

| | runs | rounds | points shared at their end | wires taken out | vias saved by the rounds' pass | nets taken out by the passes | vias as the first run ends | vias of the board |
|---|---|---|---|---|---|---|---|---|
| the rounds ended by their share | 8 of 8 | 24 to 32 | 8 to 57 | 2 to 6 | 0 in six, 2 and 17 | 9 to 83 | 317 to 375 | 276 to 370, median 302 |
| a budget of 150 million nodes, one order | | 28 | 9 | 3 | 31 | 47 | 314 | 279 |

The rounds have nine tenths of their share and what makes their routes legal the rest; the
pass for fewer vias comes after both and was begun with the share spent. The nets that are
taken out for the points still shared are the passes', which take other nets out for them:
174 in one run, for 47 where the rounds had ended. And this crate's cleanup, which is the
last of the first run, is not reached.

The rounds themselves visit as many nodes a second as cadlab's (5.2 to 5.5 million on a
thread), and neither has anything of a second thread on this board: a round's nets are
routed a few at a time only where their boxes do not meet, and of 59 nets in a late round 58
are alone (measured: 58 batches). What differs is how many rounds it takes. cadlab's router
has 2007 points shared after the first and 30 after the tenth; these rounds, with the price
of a spoke's room and no net planned on a pour of an outer layer (both kept: the first is
what leaves no pad without a spoke, the second what leaves no pad cut off its pour), have
2685 and 136, and 4 to 7 million nodes a round for 2 to 4.

### A point that stays shared costs more

Each point that is still shared after a round costs more in every round after it, by a fixed
amount for each time that it was: 0.5 in cadlab's router, which is what the rounds had. With
more, the nets that keep meeting at a point leave it sooner. `tinytapeout-demo` at a budget
of 150 million nodes, which ends no round, in eight orders (the nodes that the rounds visit
before they end, in millions; the board's vias):

| each time | nodes, by order | mean | most | rounds | open, eight orders | vias, mean | median |
|---|---|---|---|---|---|---|---|
| 0.5 | 162 186 197 127 180 144 136 203 | 167 | 203 | 23 to 40 | 1 | 276.9 | 278 |
| 1 | 119 174 97 150 179 110 123 124 | 134 | 179 | 18 to 36 | 1 | 278.8 | 275 |
| 1.5 | 99 100 116 136 133 116 202 100 | 125 | 202 | 17 to 40 | 0 | 269.0 | 263 |
| **2** | 115 105 101 136 128 100 143 123 | **119** | **143** | 20 to 29 | 0 | 271.1 | 269 |
| 3 | 96 107 95 84 118 84 80 101 | 96 | 118 | 17 to 20 | 1 | 282.0 | 278.5 |
| 4 | 116 126 85 101 137 93 116 88 | 108 | 137 | 17 to 24 | 0 | 275.9 | 273.5 |

A run at 60 s has some 150 million nodes for its rounds: with 0.5 five orders of eight need
more, with 2 none. The two other boards that the rounds are for, in four orders at the same
budget: `buspirate5-rev10` 283, 284, 288 and 301 vias and 2941, 2882, 2920 and 2972 mm with
0.5, and 284, 288, 286 and 281 and 2882, 2871, 2863 and 2897 mm with 2, the rounds 27 to 47
million nodes for 21 to 27; `lumenpnp-mobo` 326, 331, 338 and 328 vias and 5330, 5200, 5225
and 5246 mm, and 344, 328, 329 and 330 and 5348, 5240, 5246 and 5340 mm, 57 to 70 million
nodes for 47 to 68. No connection open on either with either.

A board that has no legal routing ends otherwise: on the test's board of one layer, a line
of six pads that two nets have to cross, the rounds with 0.5 end with both nets across on
one wire of the line, which is taken out alone, and the passes leave the line with its five
connections open; with 2 they end with each net across on another wire of the line, and the
two wires across are taken out, two connections open (`rounds_tests`).

### A time for the pass for fewer vias

Where the rounds' share ended them, the pass that routes each net again with vias four
times as dear now has three tenths of the time that the run has left
(`rounds::VIAS`): it takes about 2 s on `tinytapeout-demo` and saved 23 to 51 vias in each
of the 32 runs with it that were looked at. In a run with a budget of work it has what the stage has
left, as it had.

### The maps of the look

The look at the board (0.0.36) makes the maps of what is fixed on it, and the rounds made
them again: 0.72 s each time on `tinytapeout-demo`, 0.15 s on `lumenpnp-mobo`, 0.08 s on
`buspirate5-rev10`. They are now the rounds' if the board is the one that was looked at, by
its obstacles, its profiles, its grid and its layers; in a debug build they are made again
and compared. `tinytapeout-demo` and `lumenpnp-mobo` have them so. `buspirate5-rev10` does
not: the host's fill around the ways out, which is made between the two, has a fill for a
zone that had none, and that is copper to the other nets. The ways out are planned on a
board without them and make their own maps as before (0.72 s on `tinytapeout-demo`).

### What a board given up has of the time

`RouteOptions::given_up_share`: a host that gives the run with these rounds a later deadline
than the run without them says what a board that the look gives up is to have of it, and
that run then ends where it would have. cadlab gives the first run 0.78 of its budget where
it asks for the rounds, for 0.7, and tells 0.7 / 0.78.

### At 60 s

`tinytapeout-demo` through cadlab, side by side in the same minutes (open is what cadlab's
DRC counts unrouted on the board that left; the first run "ended by itself" is one that its
deadline did not end):

| set, orders | what | open, summed | vias, median | mean | first run ended by itself |
|---|---|---|---|---|---|
| C, eight | 0.0.36 | 10 | 302.5 | 302.5 | 0 of 8 |
| | a time for the pass | 7 | 306 | 307.5 | 0 |
| | the first run 0.78 of the minute | 6 | 285.5 | 302.6 | 1 |
| | both | 7 | 279 | 279.5 | 1 |
| D, eight | both | 10 | 274.5 | 282.8 | |
| | both, the first run 0.82 | 18 | 287.5 | 284.9 | |
| | both, the first run 0.74 | 13 | 299 | 297.6 | |
| | both, the pass with 0.2 | 3 | 286 | 286.1 | |
| | both, the pass with 0.5 | 5 | 277 | 282.0 | |
| J, sixteen | both, each time 0.5 | 11 | 283.5 | 287.9 | 4 of 16 |
| | **both, each time 2** | 7 | **277** | 280.4 | 10 of 16 |
| | each time 2, the first run 0.7 | 3 | 281 | 292.1 | 5 of 16 |

A run whose first run ends by itself has 253 to 290 vias; one whose first run is cut has up
to 406. With a time for the pass alone the first run's cleanup is not begun in five runs of
eight where it was in all, and the board is as it was.

This crate's own rounds on the same board, in eight orders in other minutes of the same
round: 288 vias in the median, 6 connections open in all. cadlab's own router, as its D132
has it: 273, and 47.

### Budgets of nodes

One thread, guide alone. At 40 million nodes (nine boards, three orders) 570 connections
open for 575: `tinytapeout-demo` none, none and 1 with 277, 293 and 296 vias for 2, 1 and 3
with 343, 338 and 353, its rounds ending inside the budget's share; `buspirate5-rev10` 284,
288 and 286 vias for 283, 284 and 288, `lumenpnp-mobo` 344, 328 and 329 for 326, 331 and
338, none open on either; the six others, which have this crate's own rounds or are given
up, the same to the connection and the via. At 150 million (D131's seven boards, two orders)
42 for 43, `tinytapeout-demo` none and none with 260 and 274 vias for 1 and none with 279
and 279; `glasgow-revD1` 56 and 75, as it had.

### Tried and not kept

- *The rounds with eight tenths of their share, for nine*, so that what makes the routes
  legal and the pass for fewer vias have two: 306.5 vias in the median of eight orders for
  302.5, as many connections open.
- *The price of a shared point rising twice a round, for 1.6 times*: 173, 151, 183 and 88
  million nodes to the rounds' end in four orders for 162, 186, 197 and 127, two orders
  ending with more than 50 points shared.
- *Each time 1, 1.5, 3 or 4*, above: 1.5 leaves one order of eight at 202 million nodes, 3
  ends soonest and has 282 vias in the mean for 271.
- *The first run with 0.74 or 0.82 of the minute, the pass with 0.2 or 0.5 of what is
  left*, above.

### What remains open

- `tinytapeout-demo`'s first run is still ended by its deadline in 6 orders of 16 at 60 s,
  and those runs have the most vias. The rounds have nothing of a second thread
  on a board whose nets all meet; routing several of them at once against the same board
  is another negotiation, and was not tried.
- The ways out are planned on their own maps (0.72 s on `tinytapeout-demo`), and
  `buspirate5-rev10`'s rounds make theirs again.
- Two layers, six layers and the dense boards are where they were: the rounds are not
  begun on them.

## PCB routing with the rounds of cadlab's router looked at before the run begins

0.0.34 and 0.0.35 have the negotiation of cadlab's own router as a first stage
(`Rounds::Native`, the two sections below). It was left off in cadlab for three things: a
board that the stage gave up had lost 4 to 5 s of the minute by then, the stage took longer
than this crate's own rounds, and `tinytapeout-demo` ended with more vias. This round took
them in that order. **A board is now looked at before the run plans anything for the rounds
but their ways out, and one that is given up is routed as one that never asked for them, to
the track at a budget of nodes. A route of the rounds that began on a pad of another island of
its net was handed over as joining the wrong pad, and the passes took the track out that did
join them: with that right `buspirate5-rev10`'s first run ends with every connection made.
With the passes after them, at 60 s through cadlab, `buspirate5-rev10` has 7 % fewer vias
than with this crate's own rounds in 18 to 24 s for 23 to 34 s, `lumenpnp-mobo` as many in 33
to 36 s for 51 to 54 s, and `tinytapeout-demo` 5 % less track, as many connections open and
4 % more vias, which is its first run ending at its deadline. cadlab asks for them on a board
of more than two layers.**

All of it measured through cadlab's adapter (its route_bench; at 60 s three threads a run and
four runs at a time, the load 3 to 7 on a machine of 64 threads; at a budget of nodes one
thread, where a run is the same every time). A run at 60 s ends by the clock, and two runs of
one order differ by as much as two orders do.

### The look

`rounds::look` has the board with the ways out of its rows and arrays on it, and builds what
the rounds build: the grid, what the board's copper leaves free of it, the nets as islands
to join and the cells where their pads are reached. It then tells two things, and routes
nothing:

- *How many connections have no way on the grid whatever the other nets do*, each by its own
  two islands: an end with no cell, or ends that the board's own copper keeps apart
  (`Engine::no_way`, as 0.0.35 has it inside the stage). More than one in fifty, and the
  board is given up.
- *What a first round takes*, by 48 of the connections searched for alone on the board with
  no route on it, each as often as its length is of the whole length (so that a long one,
  whose search is the dearer, is as likely as it weighs), times four: a round routes each
  connection among the routes of the nets before it, which is 1.1 to 2.5 times the search
  alone on the six boards of two layers and 2.0 to 4.8 times on the six of four and six.
  Eight first rounds with no room in the rounds' share of the time or of the work, and the
  board is given up. It is a gross measure, as the last column says, and it gives up none
  of the twelve at 60 s or at 40 million nodes that the first does not.

| board | connections | with no way | the sample alone, nodes | times four | a first round, nodes | at 60 s |
|---|---|---|---|---|---|---|
| buspirate-flash-sop | 47 | 0 | 54 k | 0.22 M | 0.09 M | |
| buspirate-rs232 | 43 | 0 | 68 k | 0.27 M | 0.08 M | |
| lumenpnp-ringlight | 37 | 0 | 15 k | 0.06 M | 0.03 M | |
| nrfmicro | 85 | 0 | 68 k | 0.27 M | 0.09 M | |
| sweep-v2.2 | 47 | 2 | 0.58 M | 2.3 M | 1.4 M | given up |
| corne-cherry | 432 | 0 | 3.3 M | 13.2 M | 5.6 M | |
| tinytapeout-demo | 464 | 0 | 0.80 M | 3.2 M | 4.3 M | |
| buspirate5-rev10 | 637 | 0 | 0.26 M | 1.0 M | 1.8 M | |
| lumenpnp-mobo | 768 | 8 | 1.6 M | 6.4 M | 6.3 M | |
| cynthion | 895 | 42 | 1.0 M | 4.1 M | 3.0 M | given up |
| glasgow-revC3 | 775 | 43 | 0.83 M | 3.3 M | 1.7 M | given up |
| glasgow-revD1 | 1918 | 57 | 5.4 M | 21.5 M | 17.4 M | given up |

(The first round's nodes are of a build that routes it whatever the look says. The look
counts a few connections more than the stage, 895 for 889 on `cynthion`, since it has the
host's fill as the board was given and the stage the fill around the ways out.)

The look charges the run none of its work. Where it gives a board up for its connections
with no way, it has searched for nothing. Inside the stage the rounds no longer give a
board up after their first round.

### A board given up is the run without the rounds

Through cadlab a board given up was not the run of a board that had never asked for the
rounds, and not only for the time: a host that fills its zones itself tells the rounds the
zones that its fill left with no copper, a pour that reaches no pad of its net yet, so that
its fill around the routes can name them once a via has joined one. This crate's own run
filled those from their outlines and planned on them: `glasgow-revC3` had 442, 458 and 411
vias at 40 million nodes for 404, 405 and 397, as it had 443 for 403.5 at 60 s in the last
section, which put that down to the time lost. **`Zones::unfilled_from` says from which zone on they are
such, and a run with this crate's own rounds, from its start or because the others gave the
board up, is the run of the design without them.**

At a budget of nodes, with the rounds asked for on every board of more than two layers:
`cynthion`, `glasgow-revC3` and `glasgow-revD1` are given up, and each of their nine runs at
40 million nodes and six at 150 million has the connections open, the vias, the track to the
tenth of a millimetre and the work that the run with this crate's own rounds has (8, 8 and
8 with 429, 410 and 423 vias; 12, 15 and 12 with 404, 405 and 397; 145, 136 and 130 with
1101, 1096 and 1117; at 150 million 8 and 8, 7 and 5, 56 and 75). At 60 s a board given up
begins its routing 1.3 to 1.9 s later than with this crate's own rounds, where it lost 4 to
5 s (`cynthion` 3.5 s into the run for 2.1 s, `glasgow-revC3` 6.0 s for 4.1 s,
`glasgow-revD1` 3.7 s for 2.4 s, in each of four orders), and ends as that run does:
`cynthion` and `glasgow-revC3` with the same connections open and the same vias in each of
the four orders (8 in each and 428, 409, 425 and 394; 9, 6, 9 and 8 and 427, 398, 392 and
409), `glasgow-revD1` with 16, 14, 7 and 8 open for 17, 16, 7 and 11.

### A route that begins on another pad of its net

The stage hands each net's route to the passes as a tree, and with it the places where its
routes end that are no pad's centre, each as a place of one of the net's terminals. A
connection is planned between two islands and searched for from every island that the wires
before it joined to its own, so it may begin on a pad of any of them. Such an end was taken
for a place of the island that the connection names. On `buspirate5-rev10`, a net of three
pads: the second pad joined to the first by a track, and the third by a route that began on
the second pad and was named for the first. The passes then had a place "of the first pad"
on the second, a track between the two that was one too many, and took it out; the first
pad was left apart on a board that the run reported whole. **`copper_at` asks every island
of the net whose copper the end is on, the one named first.**

Seven connections of `buspirate5-rev10` were open so after the first run in one order, which
cadlab then closed with a run that moves every route, 5 s of it; none in the four orders
since, and the board filled by cadlab has no line of ratsnest left in any of them where
0.0.34 had 2, 1, 3 and 1.

### What a node of the rounds is worth

A budget of nodes ends a run where its count of its own operations reaches a limit, and a
unit is to take about as long in every phase. The rounds search on maps made before them
and visit 4.7 to 6.5 million nodes a second on a thread (`lumenpnp-mobo` 4.7,
`tinytapeout-demo` 4.8, `buspirate5-rev10` 5.1, `corne-cherry` 6.5); the passes, which ask
the board about each place, do 0.5 to 0.9 million units. At a unit a node the rounds' half
of 40 million was spent on ten of `buspirate5-rev10`'s rounds, which take 43 million nodes
to end, and the run ended with 338 vias for the 317 of this crate's own rounds. **A unit is
six nodes of the rounds' search.**

| board, open and vias by order | own rounds, 40 M | `Rounds::Native`, 40 M | own rounds, 150 M | `Rounds::Native`, 150 M |
|---|---|---|---|---|
| tinytapeout-demo | 3, 7, 4; 297, 299, 348 | 2, 1, 3; 343, 338, 353 | 0, 1; 280, 332 | 1, 0; 279, 279 |
| buspirate5-rev10 | 0, 0, 0; 317, 315, 297 | 0, 0, 0; 283, 284, 288 | 0, 0; 317, 315 | 0, 0; 283, 284 |
| lumenpnp-mobo | 0, 0, 0; 343, 337, 328 | 0, 0, 0; 326, 331, 338 | 0, 0; 344, 337 | 0, 0; 326, 331 |

(Orders 0, 2 and 3 at 40 million nodes, 0 and 2 at 150 million. `tinytapeout-demo` at 150
million in eight orders with the rounds: one connection open in one, 260 to 295 vias, the
median 278.) The other boards of the two sets are given up, above, or have two layers,
where cadlab does not ask for the rounds: the totals are 575 connections open at 40 million
nodes for 583, and 43 for 43 at 150 million.

### When the rounds end

The price of a shared point rises 1.6 times a round to a thousand times a step, which it is
at from the seventeenth round, and the rounds went on to forty, or to twelve with none
better. **They now end once the price is at its most and no more than one point for every
32 connections is shared**; what is shared then is taken out a wire at a time, as before.
`lumenpnp-mobo` in one order: 17 rounds and 11 points shared in 12.7 s for 40 rounds and 9
points in 24.7 s; `buspirate5-rev10` 17 rounds and 10 points in 5.9 s for 40 and 4 in 8.7 s.
`tinytapeout-demo`'s rounds do not get there: they are ended by their share in seven orders
of eight, with 26 to 48 points shared after 24 to 30 rounds.

### With the passes after it, at 60 s

Beside this crate's own rounds in the same build and the same minutes, and cadlab's own
router (four orders, `tinytapeout-demo` eight; open by order, medians of the vias and the
track, the fastest and the slowest run; the load 6.2 to 6.7). cadlab does not make its run
for shorter ways after these rounds (its D132): it laid their routes again longer in every
run measured.

| board | own rounds | `Rounds::Native` | cadlab's router |
|---|---|---|---|
| tinytapeout-demo | 0/1/0/0/0/0/2/4; 287.5; 5025 mm; 47 to 56 s | 3/0/1/0/0/0/0/0; 299.5; 4748 mm; 46 to 58 s | 19/2/5/5/2/2/3/9; 273; 4894 mm; 34 to 61 s |
| buspirate5-rev10 | 0/0/0/0; 306.5; 2977 mm; 23 to 34 s | 0/0/0/0; 286; 2930 mm; 18 to 24 s | 4/2/4/8; 302; 3129 mm; 20 to 33 s |
| lumenpnp-mobo | 0/0/0/0; 334; 5326 mm; 51 to 54 s | 0/0/0/0; 330.5; 5234 mm; 33 to 36 s | 9/12/10/15; 339; 5446 mm; 61 to 62 s |

No run of the three boards has a pad without a spoke, in 16 orders of `tinytapeout-demo`
and four of the others, a DRC error other than unrouted, or a solder pad made solid by a
track. `tinytapeout-demo` in eight orders more: 5/1/3/0/0/0/0/3 open and 304 vias.

- *`tinytapeout-demo`'s vias are its first run's deadline.* The rounds take 24 to 28 s of
  the 42 s that cadlab gives the first run, and the cleanup after the passes, 10 to 11 s,
  is cut. In the four runs of sixteen whose first run ended by itself (those with less time
  for the rounds, below) the board has 276, 279, 282 and 287 vias; at 150 million nodes,
  where nothing is cut, 278 in the median of eight orders, for the 273 of cadlab's router.
- *`lumenpnp-mobo` has a pad apart that the run reports joined*, the same two pads of
  +3.3V in every order with the rounds (U5.21 and U5.22, 0.5 mm apart), by cadlab's DRC of
  the board as its own steps after the runs leave it: the track that is left at the second
  pad is a stub of 0.13 mm. With this crate's own rounds `buspirate5-rev10` has such in
  three orders of four (0, 1, 1 and 2) and none with these. Which step takes the rest of
  the track out was not looked into.

### What was tried and not kept

Each measured through cadlab at 60 s, `tinytapeout-demo` in eight orders and the two others
in four, beside the stage as it was in the same build.

- *The room of a spoke priced only where a route in it would leave the pad short*, counting
  the rooms that other nets' routes stand in as the rounds go: a pad of four spokes then
  has two for the tracks that pass it. `tinytapeout-demo` 288 vias in the median for 300.5
  and 301 for 303 in the mean, with 8 connections open for 4; `buspirate5-rev10` 286 for
  285.5 with 13 to 16 pads short of two spokes for 7 to 10; `lumenpnp-mobo` 338 for 329.5
  with one or two pads short for none. No fewer vias that eight orders can tell, and more
  pads short.
- *The stage without its own pass for fewer vias*: `buspirate5-rev10` 301.5 vias for 285.5,
  `lumenpnp-mobo` 345 for 329.5.
- *Rounds that their share ends with few points shared given no more time*: `tinytapeout-demo`
  8 connections open for 4, 303.5 vias and a pad with no spoke in one run of eight. *Given
  0.35 or 0.25 of what the run has left* in place of half: 3 and 7 open, 294.5 and 319.5
  vias; the first run still ends at its deadline in five and seven runs of eight.
- *The rounds' own first round as the look*, as 0.0.35 has it: it is the measure that is
  exact, and it is the round spent, 1.7 to 17.4 million nodes on the three boards that are
  given up, after everything that the run plans for it.

### What this does not show

- Two layers. With the hand-over right, `corne-cherry` still has 3, 1, 3 and 1 connections
  open and 459.5 vias with the rounds for 2, 0, 1 and 1 and 437.5 with this crate's own,
  and `nrfmicro` 44 vias for 42.5: cadlab asks for them on more than two layers only.
- `tinytapeout-demo`'s first run ending by itself: the rounds are 0.8 to 1 s each and are
  cut by their share; cadlab's own router has 0.6 s a round of the same search.
- The look's time on a board that goes on: it builds what the stage builds again after it.
  `tinytapeout-demo`, whose ten classes of net make the maps the dearest, begins its routing
  3.0 s into the run for 0.5 s with this crate's own rounds, the ways out, the look and the
  host's fill around the ways out together; the look's part of it was not told apart.

## PCB routing with the rounds of cadlab's router made legal a wire at a time

0.0.34 has the negotiation of cadlab's own router as a first stage (`Rounds::Native`, the
section below), which ended its rounds as that router does and, with this crate's passes
after it, left `tinytapeout-demo` with four to seven connections open where this crate's own
rounds leave none or one. This round looked at where those come from, stage by stage, and
changed five things in the stage. **With them `tinytapeout-demo` has eleven connections open
in sixteen runs, as with its own rounds, 5 % less track, 3 % more vias and no pad without a
spoke; `buspirate5-rev10` has 4 % fewer vias and 3 % less track in 30 to 37 s for 22 to 34 s;
and a board that the stage gives up costs 4 to 5 s of the minute, which `glasgow-revC3` and
`glasgow-revD1` pay for in vias and in connections. cadlab leaves the stage off.**

All of it measured through cadlab's adapter (its route_bench, 60 s, three threads a run, four
runs at a time, the load 4 to 7 on a machine of 64 threads; a run ends by the clock, and two
runs of one order differ by as much as two orders do).

### Where the open connections came from

- *The net with the most connections taken out whole.* The rounds end with a few points
  still shared (`tinytapeout-demo`: 4 to 20 after 33 to 40 rounds), and cadlab's router then
  takes out the net that shares the most, whole, and routes it again where nothing is
  claimed: that is the supply or the ground, whose other seventy wires were in no one's way
  and find their room taken. Five runs ended the stage with 7, 8, 13, 15 and 15 connections
  failed. **Now the wire that shares the most is taken out, of two that share as many the
  longer, and each net that lost one is routed again with the wires it kept; a connection
  that then finds no way has the wires in its way taken out for it, a wire at a time
  (`legalise_wires`, `retry_wires`).** The same rounds then end with 1, 1 and 3 failed.
- *The rounds for the pours.* Once legal, the nets whose pads the routes cut off their fills
  were planned again and negotiated for from nothing at the price of a shared point that the
  rounds ended with, a thousand times a step. On `tinytapeout-demo` each such round left
  more failed than the one before (12, 13 and 25; 8 and 22; 6 and 12) and the routes went
  back; on `buspirate5-rev10` they are 38 rounds of 68. **They are an option now,
  `RouteOptions::rounds_pours`, off by default**; the passes join such pads as they do after
  this crate's own rounds. Without them `buspirate5-rev10` ends in 28 to 39 s for 34 to 41 s
  with 291 vias for 293, and `lumenpnp-mobo` with 331.5 vias and 5309 mm for the 339 and
  5467 mm of 0.0.34.
- *A net planned on a pour of an outer layer.* With these two the stage handed over all but
  one to three connections, and the board still ended with two to nine open: every one a pad
  of the supply at the package of 0.4 mm pitch. That net has no plane, and a pour under the
  package; the stage planned its pads there as joined by the pour, or joined them to it by
  a stub of two cells, the routes of the signals went through the pour as through a plane,
  and the board filled again had the pads apart and shut in. **On more than two layers a
  net is now planned on its fills of the inner layers alone (`rounds_fills`)**, so that the
  supply's pads are routed as any net's, with their way out negotiated for. Eight orders
  then: 0/0/0/0/0/0/0/0 open.
- *A pad with no spoke.* Two to four of those eight runs, in three sets of eight, left a pad
  of the ground with no spoke: a through-hole pad with room for two, both rooms with a route
  of the stage in them, which the step for the spokes moved at the cost of two connections,
  and so did not keep. This crate's own rounds price a spoke's room; the stage did not.
  **Now it does, at `RouteOptions::spoke_cost` as they do, and the last spoke of a pad is
  kept (`kept_spokes`) from the stage's routes as from any other: the walls go up before
  its routes are laid in the world.** Four sets of eight orders with the price: no pad
  without a spoke in 32 runs. With the price for the spokes that are held alone, four runs
  of eight had such a pad, and with the walls alone two of eight.
- *A board given up after its first round.* That round said whether the grid reaches the
  pads; `Engine::no_way` now says it before anything is routed, of each connection by its
  own two islands (41 of 868 on `cynthion` for the 32 that the round found, 42 of 753 on
  `glasgow-revC3` for 40, 56 of 1914 on `glasgow-revD1` for 52, 8 of 721 on `lumenpnp-mobo`
  for 8 and none on the other two), 1.2 s into the stage.

### The boards of more than two layers, the stage beside this crate's own rounds

Open by order; medians of the vias and the track; the time of the fastest and the slowest.

| board | own rounds: open; vias; track; time | `Rounds::Native` as it is now |
|---|---|---|
| `tinytapeout-demo`, eight orders | 0/1/0/0/0/0/0/4; 288; 5079 mm; 53 to 57 s | 5/1/0/0/0/1/0/0; 297; 4809 mm; 53 to 58 s |
| the same, another set (its own rounds: the round before) | 0/1/0/0/0/0/1/4; 287.5; 5034 mm | 0/2/0/0/0/0/0/2; 293; 4792 mm; 55 to 58 s |
| the same, with the ways out planned on the fills as the stage counts them | | 3/1/0/0/3/0/2/2; 306; 4811 mm; 54 to 59 s |
| `buspirate5-rev10` | 0/0/0/0; 306.5; 2977 mm; 22 to 34 s | 0/0/0/0; 294; 2890 mm; 30 to 37 s |
| `lumenpnp-mobo` | 0/0/0/0; 334; 5326 mm; 52 to 55 s | 0/0/0/0; 332; 5208 mm; 54 to 58 s |
| `cynthion`, given up | 8/8/8/8; 417; 3032 mm; 36 to 42 s | 8/8/8/8; 417; 2903 mm; 40 to 49 s |
| `glasgow-revC3`, given up | 9/6/9/8; 403.5; 3643 mm; 59 s | 5/8/8/6; 443; 3681 mm; 59 s |
| `glasgow-revD1`, given up | 9/14/6/12 (the round before) | 10/35/30/30; 1171.5; 9898 mm; 55 to 57 s |

No run of either has a pad without a spoke, a DRC error other than unrouted, or a solder pad
made solid by a track.

- *The time.* On `tinytapeout-demo` the stage's rounds end by its share, not by themselves:
  0.7 to 0.9 s a round, some four million nodes whatever the number of nets routed again (57
  nets of which 14 share a point, late), because the searches of the nets that share, at a
  thousand times a step, visit the board. The first run then ends at its deadline, 42 s,
  where with this crate's own rounds it ends by itself after 22 to 33 s, and the runs after
  it have 15 s for 25 to 35.
- *A board given up* begins its run again, escapes and planes first: 4 to 5 s of the minute
  (`cynthion`: the first run over at 28 s for 20 s), of which the stage's own part is 1.2 s.

### Tried and not kept

Each on `tinytapeout-demo` at 60 s, four or eight orders, beside the stage as it was.

- *The rounds ended where the price of a shared point is at its most*, round 19, or at
  round 12, so that the passes have the time: 2/3/6/9/4/6/4/0 and 4/2/7/10/3/2/9/4 open
  for 2/5/4/8.
- *A wire kept wherever it is far from a shared point*, for every net and not only those of
  64 connections: the rounds cost the same nodes (22 million after five rounds for 24,
  71 for 79 after seventeen), since it is the searches of the nets that share that cost.
- *The pads cut off their fills told early*, after rounds 2, 5, 9 and 14, on the routes as
  they are: the ground was planned again each time with its 124 connections, and the board
  ended with 7/10/3/12 open.
- *The pads cut off joined by wires at the stage's end*, each net planned again keeping its
  wires and the new connections searched for where nothing is claimed, with the wires in
  their way taken out: the pads are shut in by then (10 and 12 failed for 6 and 6), and it
  took 2 to 10 s.
- *This crate's own escapes* in place of the stage's ways out, with the rounds ended at
  round 19: 3/3/1/5/3/2/0/1 open, 272.5 vias, and a pad with no spoke in three runs of
  eight.

### What this does not show

- The stage at a budget of nodes: every figure here is of runs that the clock ends.
- The boards of two layers, where a net is still planned on its pours and the fills that
  are no planes are copper to every other net: not looked at this round.
- Whether the ways out planned on the fills as the stage counts them cost `tinytapeout-demo`
  the vias and the connections of the third line: it is one set of eight runs beside two,
  and the release plans them on the fills as 0.0.34 did.
- The 72 cells beside cadlab's own router: not measured with the stage, which cadlab
  leaves off.

## PCB routing with the rounds of cadlab's own router

What was built after 0.0.33. Six releases took from cadlab's router what it does better,
a piece at a time, and each explained a part of what its negotiation ends with and this
crate's does not: its first round has a fifth fewer vias and a tenth less track, and its
rounds end where this crate's go on. So its negotiation itself is here now, whole, as a
first stage that a host can ask for: `RouteOptions::rounds`, `Rounds::Native`. **It ends
its rounds as cadlab's router ends them, round for round on two boards of four and within
a few vias in a hundred on the others. It is off by default, and cadlab leaves it off:
with the passes after it `tinytapeout-demo` and `buspirate5-rev10` have some 5 % fewer
vias and 6 % less track, the first with four to seven connections open for none or one
and the second in half as much time again, and `lumenpnp-mobo` has more of both.** Measured on 2026-11-23 and
2026-11-24 through cadlab's adapter and its `route_bench`, with a checkout of this crate
in place of the release and prints in both routers that are in no release; three threads
a run, the load that `uptime` gives 3.4 to 7.7 with two to four runs at a time.

### What it is

The stage is cadlab's `router::{engine, grid, index, pours, fanout, components, scoped}`
as they are, MIT as this crate is and of the same authors, with its own view of the board
made from a [`Design`](../src/pcb/mod.rs) in place of cadlab's project (`pcb::rounds`):

- *The grid*: half a track and its clearance of the narrowest net routed, snapped to the
  pitch that the most pads of rows of fine pitch share where that is within 6 %, and
  placed so that the most of those pads are on its lines.
- *The board*: for each class of net by width, clearance and via, what the board's copper,
  holes, keepouts and edge leave free of the grid, on the points of a grid of half the
  pitch for tracks and on the cells for vias; a point that only one net's copper is in
  the way of is that net's. A rule area's clearance is between two pieces of copper that
  both touch it and a pad's own replaces the nets', as [`geom::drc`](../src/geom/drc.rs)
  has them.
- *The nets*: the copper that touches is one island, the pads that a fill joins among it,
  and the islands are joined in a tree from the first, each to the nearest by the distance
  between pads as the crow flies. A connection may end on any cell of a fill of the island
  that it joins, within 4 mm of its end.
- *The widths*: a net is no wider than leaves each pad of its between the other pads of
  the pad's part, and than two tracks out of two pads of a row clear each other at as the
  grid has them (`rounds::necks`), in every run of a board whose first run has these
  rounds: the routes of one run are laid again in the next.
- *The ways out*: a dog-bone for a ball inside an array, with the smallest via of the
  rules where the net's own does not fit, and an escape for a pad of a row that no line
  of the grid leaves (`rounds::fanout`), fixed copper of their nets before anything is
  routed, in place of this crate's own escapes. Both need the parts of the pads, which a
  design now has (`Design::parts`).
- *The rounds*: every net once, the shortest first, in batches of nets whose boxes do not
  meet, on as many threads as there are with the same routes on any number; then the nets
  with a point shared and those near one, a net of 64 connections or more keeping its
  wires away from them; a shared point costs 0.6 of a step at first and 1.6 times that
  each round up to a thousand, and half a step more for each round it was shared in;
  forty rounds, or twelve with none better. What is still shared is taken out, the net
  with the most shared first, and routed again where nothing is claimed.
- *The pours*: with a host that fills its zones around the routes
  (`RunControl::refill`), the nets whose pads the legal routes cut off their fills are
  planned again from what the fills still join and negotiated for, three times at most,
  and the routes go back where that leaves more open. A first negotiation still going at
  two thirds of the stage looks at what it cuts off, once.
- *After them*: a connection that found no way has the nets in its way taken out with
  it, and all are routed again, kept with fewer open (six rounds); and each net with a
  via is routed again where the others are with a via at four times its price, kept with
  fewer vias and a tenth more track at most.
- *What it gives*: each net's route as a tree, as the negotiation of this crate gives its
  own, and the places where its routes end that are no pad's centre: on a pad's copper,
  on a fill, on fixed copper of the net.

Three things are not cadlab's, where this crate's passes have other rules and what was
handed over would be taken out again: a via does not touch a pad of its own net (cadlab's
router has them a tolerance apart), an area's clearance holds a via only where the area
is on every layer, and the fills of the zones that are no planes are copper to the other
nets, as they are to the passes, where cadlab's router lets a route through a pour and
has it filled again. And a pad is at the centre of its shape here, and at its footprint's
place there.

### The rounds beside cadlab's

Both routers print each round: the nets routed again in it, the points shared after it,
the vias and the track of the routes as they stand. One run of each at a time, cadlab's
with seed 1 at 60 s and the stage with the same seed in a run of 120 s, its share 42 s
where cadlab's router has 45 s for its rounds; the nets planned on every fill, as
cadlab's are (`RouteOptions::outer_pours`).

*With the three rules above as cadlab has them* (a build before they were changed):
`corne-cherry` and `buspirate5-rev10` are the same in every round, to the via and the
tenth of a millimetre, and through the rounds for the pours.

| round | `corne-cherry`, cadlab: nets, shared, vias, mm | the stage | `buspirate5-rev10`, cadlab | the stage |
|---|---|---|---|---|
| 1 | 152, 4792, 238, 7150.6 | 152, 4792, 238, 7150.6 | 183, 2645, 115, 2812.3 | 183, 2645, 115, 2812.3 |
| 2 | 137, 2220, 292, 7244.4 | 137, 2220, 292, 7244.4 | 155, 1151, 166, 2829.2 | 155, 1151, 166, 2829.2 |
| 5 | 112, 712, 401, 7359.6 | 112, 712, 401, 7359.6 | 88, 153, 250, 2860.8 | 88, 153, 250, 2860.8 |
| 12 | 68, 60, 478, 7966.5 | 68, 60, 478, 7966.5 | 26, 51, 254, 2889.2 | 26, 51, 254, 2889.2 |
| the rounds end | after 33: 492 vias, 8007.2 mm, 1 open | after 33: 492, 8007.2, 1 | after 29: 257, 2869.0, 3 | after 29: 257, 2869.0, 3 |
| after the rounds for the pours | after 38: 496, 8089.8, 1 | after 38: 496, 8089.8, 1 | after 68: 312, 3085.0, 3 | after 68: 312, 3085.0, 3 |

`tinytapeout-demo`, whose rule areas this crate's rules have as one clearance each where
cadlab has the rules themselves, and `glasgow-revD1`, some of whose pads are not at the
centre of their shape, which moves the grid by 72 µm:

| round | `tinytapeout-demo`, cadlab | the stage | `glasgow-revD1`, cadlab | the stage |
|---|---|---|---|---|
| 1 | 137, 2007, 184, 4494.2 | 137, 2284, 185, 4494.0 | 561, 8013, 743, 8778.1 | 561, 7741, 722, 8688.0 |
| 2 | 104, 895, 206, 4519.1 | 108, 948, 210, 4537.5 | 359, 3803, 824, 8900.5 | 354, 3813, 805, 8797.7 |
| 5 | 85, 178, 238, 4572.7 | 79, 215, 237, 4580.4 | 245, 918, 939, 9123.1 | 264, 1111, 927, 9024.3 |
| 9 | 55, 57, 248, 4637.7 | 63, 50, 246, 4639.0 | 188, 387, 998, 9400.5 | 204, 500, 972, 9336.2 |
| the rounds end | after 40: 251, 4615.3, 2 open | after 33: 261, 4743.0, none | after 9, at their share: 956, 8984.0, 92 | after 9: 941, 9033.9, 93 |

*As released*:

| board | cadlab: rounds; vias, mm, open as they end | the stage | first round, cadlab | the stage |
|---|---|---|---|---|
| `tinytapeout-demo` | 40; 251, 4615.3, 2 | 40; 250, 4552.7, 3 | 184, 4494.2 | 185, 4503.2 |
| `corne-cherry` | 33; 492, 8007.2, 1 | 40; 550, 8494.4, 3 | 238, 7150.6 | 238, 7165.8 |
| `buspirate5-rev10` | 29; 257, 2869.0, 3 | 27; 255, 2901.4, 4 | 115, 2812.3 | 113, 2814.9 |
| `glasgow-revD1` | 9; 956, 8984.0, 92 | 9; 967, 9080.3, 76 | 743, 8778.1 | 723, 8678.7 |

`corne-cherry` is the board that the fills as copper cost: its teardrops are zones of two
pads, which the routes of cadlab's router cross and the stage's go round, and its rounds
then have 191 points shared after the twelfth for 60.

### With the passes after it

The nets planned as this crate plans them (a net with a plane on its planes alone, each
pad with its own stub), which leaves fewer connections open than cadlab's plan: with that
`tinytapeout-demo` had 6 open in one order and 15 without the rounds for the pours, pads
of the supplies that the routes shut in, for 1. At 60 s over four orders, the stage beside this crate's own rounds, the same
build, two orders of each at a time; open by order, medians of the vias and the track:

| board | own rounds: open; vias; track; time | `Rounds::Native` |
|---|---|---|
| `tinytapeout-demo` | 0/1/0/0; 284.5; 5038 mm; 53 to 56 s | 5/5/4/7; 271.5; 4723 mm; 55 to 57 s |
| `buspirate5-rev10` | 0/0/0/0; 307.5; 3056 mm; 20 to 33 s | 0/0/0/0; 293; 2871 mm; 34 to 40 s |
| `lumenpnp-mobo` | 0/0/0/0; 334; 5300 mm; 50 to 54 s | 0/0/0/0; 339; 5467 mm; 57 to 58 s |

Open is what the router reports; on the board as cadlab fills it after the run,
`buspirate5-rev10` has 2, 1, 3 and 1 connections apart with the stage for 0, 0, 1 and 2,
and `lumenpnp-mobo` 1, 0, 0 and 0 for none. Two of the twelve runs with the stage have a
pad with no spoke. With this crate's own
escapes in place of the stage's ways out (`RouteOptions::rounds_fanout` off),
`tinytapeout-demo` has 1/0/3/2 open, 281.5 vias and 4741 mm, and `buspirate5-rev10`
294.5 vias and 2883 mm. At 120 s, one order, the nets planned on every fill:
`tinytapeout-demo` none open, 261 to 264 vias and 4689 to 4751 mm in 86 to 90 s, where
cadlab's router has 273 and 4894 mm with four open at 60 s; `buspirate5-rev10` none open,
288 to 303 vias and 2755 to 2799 mm.

What stays open are pads that the stage's routes shut in: of the 458 connections of
`tinytapeout-demo` it hands over all but three to eight, and the passes complete those
on a board that has no room left by the pads, where this crate's own rounds, at twice
the pitch, leave it. And its rounds take the time: 30 to 40 of them are 18 to 28 s of the
42 s that the first run has.

*Which boards it is begun on.* The stage gives a board up after its first round, and the
run begins again with this crate's own rounds, necks and escapes
(`Stats::rounds_given_up`), if eight times what that round took is more than its share,
or if more than one connection in fifty has no way on its grid whatever the other nets
do: `cynthion` (2.6 s of 19 s, 32 of 868), `glasgow-revC3` (40 of 753) and
`glasgow-revD1` (4.2 s of 20 s, 52 of 1914) are given up, at 5 s of the minute, and end as
with the own rounds but for that time (`glasgow-revD1` 25 open for 13 in one order). On
two layers this crate's own rounds are at the pitch already: `corne-cherry` 2 to 8 open
and 434 to 498 vias with the stage for 1 and 437, `nrfmicro` 51 vias for 42.5.

### What this does not show

- The twelve boards beside cadlab's router with the stage: it is not what cadlab runs.
- A budget of nodes with the stage: its nodes are those of its searches, and the grid's
  maps are not counted.
- A host with no parts told: the stage then finds no rows, plans no way out and narrows
  no net.

## PCB routing with the open connections negotiated on the finer grid

What was built after 0.0.32, for what
[the release before](#pcb-routing-with-the-pours-filled-around-legal-routes) measured and
left: the nets without a plane are given to the passes with 174 to 245 vias and have 258
to 326 after them, for 10 to 17 connections completed by 44 to 219 nets taken out. So
those connections are negotiated for instead, on the grid of the pitch, by the nets
concerned: `RouteOptions::whole_finer`. **It is off by default, and cadlab leaves it off:
it halves what the passes are given and the nets they take out, and the board ends as it
did.** Measured on 2026-11-23 through cadlab's adapter and its `route_bench`, with a
checkout of this crate in place of the release and prints that are in no release; 60 s
runs two orders of each of two builds at a time, three threads each, the load 4.3 to
8.9; budgets of nodes on one thread.

### What it is

When the rounds at twice the pitch end, what is still shared is taken out and routed
again where nothing is claimed (`whole::legalise`), and each net is routed once more
for fewer vias. A net that was taken out and found no way has a connection open on the
grid. With `whole_finer` the nets are then put as they are on a grid with a cell between
every two of theirs (`whole::refined`: the same copper, a node between every two of a
layer), and there are rounds on it (`whole::finer_rounds`): the nets with a connection
open are routed across the others at eight steps a claimed point; the nets whose claims
they are in are routed again in the next round, with their wires that are in no one's
way kept, at a price that rises by 1.6 a round; sixteen rounds at most, five since the
best, within a tenth of the time and of the nodes that the run has left. The routes go
back to the round with the fewest points shared, what is still shared there is taken
out as before, and the routes of that grid are the negotiation's only if fewer
connections are open on it than before the rounds (`Stats::whole_finer`).

### Stage by stage

The default order at 150 million nodes, one thread; vias and mm of track of the routes
as they stand.

| stage | `tinytapeout-demo`, 0.0.32 | with `whole_finer` | `buspirate5-rev10`, 0.0.32 | with `whole_finer` |
|---|---|---|---|---|
| after the first round | 248, 4994 | the same | 269, 3191 | the same |
| the rounds end: rounds, points shared, nets taken out | 109, 20, 5 | the same | 40, 2, 1 | the same |
| legal, after the pass for fewer vias; connections open on the grid | 350, 5564; 12 | the same | 410, 3603; 4 | the same |
| the rounds on the grid of the pitch: rounds, open before and after; nodes | | 10, 12 and 8; 7.5 million, their tenth | | 8, 4 and 3; 0.5 million |
| given to the passes: vias; connections apart | 339; 10 | 350; 10 | 402; 1 | 403; 0 |
| after the passes: vias, track; nets taken out | 365, 5774; 44 | 361, 5793; 0 | 407, 3626; 1 | 406, 3628; 0 |
| after each net again and the stubs to the fills | 337, 5631 | 324, 5651 | 400, 3512 | 399, 3523 |
| the first run ends | 326, 5559 | 317, 5597 | 387, 3495 | 387, 3499 |
| the board, after the later runs | 280, 4886 mm, none open | 277, 4885 mm, none open | 317, 2963 mm, none open | 314, 2948 mm, none open |

`tinytapeout-demo` in eight orders at 150 million nodes: the connections open on the grid
go from 11 to 21 to 3 to 9 in 5 to 14 rounds, the passes are given 2 to 10 apart for 8
to 15 and take 406 nets out over the eight orders for 852; the boards have 298 vias and
4998 mm with none open in any order, for 296.5 and 5024 mm with one open in one (medians).
Halving what the passes take out does not move the board: what a net taken out comes back
with is taken out again by the end of the first run and by the runs after it, with the
rounds or without.

### What it ends with

At 60 s, 0.0.32 beside `whole_finer`, four orders and `tinytapeout-demo` eight; medians of
the vias and the track, connections open by order:

| board | 0.0.32: open; vias; track | with `whole_finer` |
|---|---|---|
| `tinytapeout-demo` | 0/1/0/0/0/0/2/4; 287.5; 5044 | 0/0/2/0/0/0/2/4; 280; 5003 |
| `buspirate5-rev10` | 0/0/0/0; 306.5; 2977 | 0/0/0/0; 309.5; 2976 |
| `lumenpnp-mobo` | 0/0/0/0; 334; 5326 | 0/0/0/0; 335; 5369 |
| `cynthion` | 8/8/8/8; 417; 3032 | 8/8/8/8; 419.5; 3026 |
| `glasgow-revC3` | 9/6/9/7; 403.5; 3635 | 8/5/7/5; 413.5; 3732 |
| `glasgow-revD1` | 15/30/7/8; 1182; 10615 | 16/23/7/8; 1196.5; 10577 |

No pad with no spoke in either, in any order. The connections open on the grid go from
11 to 19 to 3 to 11 on `tinytapeout-demo`, from 20 to 23 to 11 to 13 on `glasgow-revC3`
and from 11 to 8 on `lumenpnp-mobo`; `glasgow-revD1`, whose rounds its share ends, has
none or one round of them. `glasgow-revC3` has 25 connections open over the four orders
for 31, with 2.5 % more vias and 2.7 % more track, of which the connections made are a
part; `tinytapeout-demo` 2.6 % fewer vias and 0.8 % less track with 8 open for 7. Against
cadlab's router that moves no measure of any board from worse to equal, and
`glasgow-revC3`'s vias from better to equal, so cadlab leaves it off.

Budgets of nodes, one thread, connections open over the boards and orders: 150 million,
seven boards and two orders, 43 without and 39 with (`tinytapeout-demo` none and none for
none and 1, with 277 and 307 vias for 280 and 332; `glasgow-revC3` 5 and 4 for 7 and 5,
with 403 and 399 vias for 409 and 385; the others as they were, within seven vias);
40 million, nine boards and three orders, 583 and 586 (`tinytapeout-demo` 3, 9 and 8
for 3, 7 and 4; `glasgow-revC3` 12, 15 and 9 for 12, 15 and 12; the others as they
were). Without it the 41 runs have the connections, vias and track of 0.0.32. No pad
with no spoke in any of the 82 runs.

### What was tried and not kept

- *The rounds on the grid of the pitch before anything is taken out*, from the routes of
  the best round with their points shared, and the pass for fewer vias on that grid
  after them: `tinytapeout-demo` at 150 million nodes over eight orders 288 vias and
  4996 mm with none open at eight steps a claimed point and 294 and 5015 mm at thirty,
  for 296.5 and 5024 mm with one open; at 60 s 279 vias for 287.5 with 13 connections
  open over the eight orders for 7 and a pad with no spoke in two of them, the rounds
  24 million nodes and 10 s in one order with 58 points shared before them and 19 at
  best; `buspirate5-rev10` 323.5 vias for 306.5, the pass for fewer vias on the finer
  grid keeping 19 where it kept 56 (its slack is in steps, which are half as long, and
  its nodes those of the coarser grid); `glasgow-revC3` 19 open over four orders for
  32 with 3.5 % more track.
- *A first price of fifty steps and twenty-four rounds*: `tinytapeout-demo` at 150 million
  nodes 292 vias and 4968 mm with one open in one order, for 298 and 4998 mm with none.
- *The same rounds after a negotiation at the first pitch*, on two layers, from the pitch
  to half of it: `corne-cherry` at 60 s over eight orders 9 connections open for 9, 432.5
  vias for 430.5, 34 to 55 s for 34 to 56 s.
- *The first pitch on more than two layers where the first round says there is room*,
  and twice the pitch only where not: `tinytapeout-demo` at 60 s over eight orders 321
  vias for 287.5, 21 connections open for 5 and a pad with no spoke, its rounds ended by
  their share at the eighteenth to the sixty-third with 23 to 126 points shared;
  `buspirate5-rev10` 3136 mm for 2977 and 50 to 57 s for 23 to 33 s.
- *`legal_pours` with a look at the pours late in the share on a board with no inner
  plane*, which had `corne-cherry` at no connection open in four orders in the release
  before: over eight orders at 60 s 12 open for 9 and 450 vias for 426.5.

### What this does not show

- A gain. The rounds do what they were built for, and the measure they were built to
  move does not move.
- More than four orders at 60 s on the boards other than `tinytapeout-demo`, and more
  than one period.
- Why the board ends as it did: by the stages above the first run's own end, each net
  routed again and cleanup, takes out of both what the passes added, and the later runs
  take the boards to the same vias. Where the later runs get them was not looked at.

## PCB routing with the pours filled around legal routes

What was built beside [the change below](#pcb-routing-with-a-pour-left-whole), after 0.0.30,
for the largest difference that the first round of the two routers has
([below](#the-first-round-of-both-routers-connection-by-connection)): cadlab's router takes
the pads that a pour of an outer layer joins as joined, and this crate plans a net with a
plane on its inner planes alone. Planned on every pour it lost twice, and the reading was
that the pads cut off are told on routes that are not legal. So the order of cadlab's
router was built, `RouteOptions::legal_pours`, and the planning,
`RouteOptions::outer_pours`. **Both are off by default, and cadlab sets neither: with the
order built, planning on outer pours still loses, and the order alone gains nothing.**
Measured on 2026-11-23 through cadlab's adapter and its `route_bench`, with a checkout of
this crate and switches and prints that are in no release; with both off the nine boards at
40 million nodes and the seven at 150 million route to the same connections open and the
same vias as 0.0.30, in every order.

### Stage by stage

`tinytapeout-demo`, the default order, 150 million nodes, one thread; vias and mm of track
of the routes as they stand. cadlab's router beside them, one run at 60 s (its decision
D122): 255 vias and 4686 mm as its rounds end, 251 and 4615 made legal, 287 and 4899 after
its rounds for the pours, 248 and 5013 after its pass for fewer vias, 244 and 4947 at the
end, with 19 connections open.

| stage | 0.0.30 | `legal_pours` | `legal_pours` and `outer_pours` |
|---|---|---|---|
| connections planned, vias and track after the first round | 458; 248, 4994 | 458; 248, 4994 | 366; 178, 4843 |
| first time the rounds end: round, points shared, connections open on the fill of the legal routes, pieces cut off | 31, 32, not counted | 31, 32, 28, 8 | 40, 30, 61, 35 |
| second | | 60, 27, 14, 1 | 64, 65, 22, 6 |
| third | | 73, 35, 14, 1 | 98, 31, 24 for 22: back to the second |
| fourth | | 96, 31, 19 for 14: back to the third | |
| the negotiation's routes, after its pass for fewer vias | 109 rounds; 350, 5564 | 96 rounds; 310, 5429 | 98 rounds; 250, 5307 |
| given to the passes: vias, of them on nets with planes; connections apart | 339, 110; 10 | 299, 109; 12 | 245, 59; 14 |
| after the passes: vias, track; rip-ups | 365, 5774; 44 | 372, 5868; 82 | 298, 5695; 59 |
| after each net again and the stubs to the fills; pieces cut off told in the run | 337, 5631; 26 | 351, 5688; 14 | 293, 5668; 66 |
| the first run ends | 326, 5559 | 330, 5584 | 285, 5571 |
| the board, after cadlab's later runs | 270, 4918 mm, none open | 290, 5128 mm, none open | 282, 5193 mm, none open |

Planned on outer pours the nets with planes begin with 59 vias for 110, and the board is
ahead by 41 vias as the first run ends. cadlab's run for shorter ways then finds 204 mm
and 13 vias on the board of the first column and 7.5 mm and no via on that of the third,
102 nets left as they were, and the fill of the routed board makes 44 of the first
column's stub vias needless and 12 of the third's. What the planning saves in stubs the
end of the run takes out anyway, and what it costs in track stays. `buspirate5-rev10` the
same way: 169 vias after the first round for 269, 62 vias on nets with planes given to
the passes for 165, of which the fill makes 12 needless for 69; 307 vias and 3028 mm at
the end for 317 and 2996 mm, and 19 pads short of two spokes for 6.

### What it ends with

At budgets of nodes, one thread (connections open by order):

| | 0.0.30 | `legal_pours` | both |
|---|---|---|---|
| nine boards, three orders, 40 million | 583 | 582 | 606 |
| `tinytapeout-demo` | 3, 7, 4 | 5, 7, 4 | 8, 23, 23 |
| `corne-cherry` | 36, 28, 22 | 36, 28, 22 | 31, 29, 20 |
| `lumenpnp-mobo` | 0, 0, 0 | 0, 0, 0 | 0, 6, 0 |
| `cynthion` | 8, 8, 8 | 8, 8, 8 | 9, 8, 8 |
| `glasgow-revC3` | 12, 15, 12 | 12, 15, 9 | 13, 11, 9 |
| `glasgow-revD1` | 145, 136, 130 | 145, 136, 130 | 150, 119, 130 |
| seven boards, two orders, 150 million | 43 | 39 | not measured |

`legal_pours` at 150 million: `tinytapeout-demo` none and none open for none and 1, with
290 and 272 vias for 270 and 332; `glasgow-revC3` 4 and 5 for 7 and 5, with 413 and 407
vias for 409 and 384; `lumenpnp-mobo` 343 and 336 vias for 334 and 336; `cynthion` 422
and 418 for 426 and 410; `buspirate5-rev10` 317 and 319 for 316 and 317; the others as
they were. `tinytapeout-demo` over eight orders at 150 million, on a build with the
switches: none open in any for one in one, 289.5 vias and 5016 mm for 292.5 and 5050
(medians), 38 pads short of two spokes in all for 47.

At 60 s on three threads, one run of each side by side, four orders, `tinytapeout-demo`
eight (the load 4.3 to 10.7, median 6.0); medians of the vias and the track:

| board | 0.0.30: open; vias; track, mm; pads short of two spokes | `legal_pours` |
|---|---|---|
| `corne-cherry` | 7, 0, 1, 1; 434; 7565; 6 | 5, 0, 1, 2; 429; 7565; 6 |
| `tinytapeout-demo` | 0, 1, 0, 0, 0, 0, 0, 4; 285.5; 5054; 46 | 0, 0, 0, 1, 1, 0, 0, 0; 293; 5179; 39 |
| `buspirate5-rev10` | 0, 0, 0, 0; 308.5; 3018; 33 | 0, 0, 0, 0; 309; 3027; 37 |
| `glasgow-revC3` | 9, 6, 9, 8; 403.5; 3663; 0 | 7, 8, 8, 6; 411.5; 3756; 0 |
| `glasgow-revD1` | 17, 14, 12, 12; 1169; 10756; 50 | 17, 16, 7, 10; 1182; 10747; 50 |
| `lumenpnp-mobo` | 0, 0, 0, 0; 335; 5407; 0 | 1, 0, 0, 0; 335; 5407; 0 |
| `cynthion` | 8, 8, 8, 8; 417; 3069; 4, in 40 to 45 s | 8, 8, 8, 8; 410.5; 3055; 4, in 50 to 53 s |

So at 60 s `legal_pours` has 2.6 % more vias and 2.5 % more track on `tinytapeout-demo`,
2.5 % more track on `glasgow-revC3`, a connection open in one order of `lumenpnp-mobo`
and 8 to 10 s more on `cynthion`, for a few connections on the two glasgows and
`tinytapeout-demo` that the same code moves from one measurement to the next. No pad of
any board has no spoke with it.

Both, beside cadlab's router in the period after (the load 5.5 to 11.6, median 7.4):
`glasgow-revD1` 42, 49, 71 and 54 connections open, where 0.0.30 has 12 to 17, with 1131
vias and 10187 mm; in the one run looked at its rounds are at their eleventh at 0.75 of
their share with 551 points shared and 64 pieces of fill cut off, and making those
routes legal takes what is left of the share. `lumenpnp-mobo` 0, 6, 0 and 0 open;
`cynthion` 10, 8, 10 and 8; `tinytapeout-demo` none open in eight orders with 296.5 vias
and 5221 mm; `buspirate5-rev10` 305 vias and 3074 mm with 51 pads short of two spokes;
`glasgow-revC3` 419 vias and 3840 mm; **and pads with no spoke**: `cynthion` 2 in one
order, `glasgow-revD1` 1 in each of three, where 0.0.30 has none on any board.

### What was tried and not kept

`tinytapeout-demo` at 150 million nodes over eight orders unless said, on builds with a
switch for each.

- *The rounds going on from the legal routes*, as cadlab's router has it: a net taken out
  that found no way where nothing is claimed has no round again; 16 connections apart as
  the passes begin for 10 and 202 rip-ups for 44 in the default order, 291 vias for 270.
  Over the eight orders 292.5 vias and 5047 mm with 36 pads short, none open. With
  `outer_pours`: 308.5 vias, 5310 mm, 57 pads short, one open.
- *The nets that making the routes legal left open routed in the rounds for the pours
  too*: with `outer_pours` 285 vias, 5238 mm, 4 open over the eight and a pad with no
  spoke; without, 292 vias and 5027 mm.
- *The pass for fewer vias before each fill*, as cadlab's router has its own before its
  last rounds for the pours: with `outer_pours` 295.5 vias, 5186 mm and 70 pads short.
- *`outer_pours` with the pads cut off told on the routes as they are*, 0.0.30's order:
  304.5 vias, 5177 mm, 2 open, 63 pads short; `buspirate5-rev10` 319.5 vias over four
  orders for 309.5, one connection open.
- *No look at the pours late in the share*: with `outer_pours` and the rounds going on
  from the legal routes, `tinytapeout-demo` at 40 million nodes 27, 24 and 33 open; with
  the look 26, 15 and 10, and as released 8, 23 and 23.

### What this does not show

Why the routes of the other nets are longer once the pours are terminals: 4434 mm for the
nets with no zone on the board of the first column and 4653 mm on one planned on outer
pours, with 205 and 216 vias (one order each). cadlab's router lays those nets through
the same pours and ends with less track; its rounds are at half the pitch, and its pours
are cells that a route may end on anywhere near, where a net here has eight places beside
each pad cut off. Neither was changed. `outer_pours` at 150 million nodes as released,
the generated boards, and more than one period at 60 s were not measured.
## PCB routing with a pour left whole

What was changed after [the release below](#pcb-routing-with-a-pads-last-spoke-kept),
0.0.30, with the same aim: level with cadlab's own router or ahead of it on every measure
on every board of its corpus ([decisions](architecture.md#decisions)). It is still not
met. Measured on 2026-11-23 through cadlab's adapter and its `route_bench`, with a
checkout of this crate before it was released, and with prints and switches in both
routers that are in no release, on a machine that another job kept busy (its load 5 to
12): the times are of runs side by side in the same minutes, and what is at a budget of
nodes is on one thread, which the load does not move. The table beside cadlab's router at
60 s is in cadlab's `docs/ROUTER.md` and its decision D127.

### The route that cut a pad off its pour

`buspirate-flash-sop`, two layers and a ground poured on both, had five vias in three
orders of four and six in the fourth, for the four of cadlab's router. The fifth is a
via beside a capacitor's pad of the ground: the pad is on the top layer, in the pour, and
the routes of four other nets close a ring round it, of tracks and of the pads of two
connectors between whose pins they pass. cadlab's router leaves that pour in one piece by
the way its routes happen to go; it has no rule for it either.

In a run for shorter ways each such stub is now looked at
([how](architecture.md#decisions)): `pcb::uncut`. Two things had to be found on the
way. The fills that a host gives are around the routes as they were when it filled: the
cells of the pour have no fill where a route is, whether that route is taken as not there
or not, so a cell that the routes of the nets asked about took is given back where the
world says that copper of the pour's least width has room there with those nets gone
alone. And the nets routed again are pulled tight while the way of the fill is still
kept: without it they are back on the straight line they had. One net was enough on this
board in the three orders (`VCC_CHIP`, 68.4 mm for 66.3); in the fourth the pad's
piece of the pour holds a second pad with a via of its own, and the one way that the fill
has costs the net that is in it two vias, for the one saved: not kept.

| `buspirate-flash-sop`, four orders, 60 s | vias | track | outside the pads |
|---|---|---|---|
| 0.0.30 | 6, 5, 5, 5 | 439.7 to 440.4 mm | 428.4 to 428.8 mm |
| with the pour left whole | 6, 4, 4, 4 | 438.9 to 441.8 mm | 428.1 to 430.9 mm |
| cadlab's router, four seeds | 4, 4, 4, 4 | 460.2 to 461.7 mm | 430.6 to 432.0 mm |

### A run's end at a pad, and the price of a via in a run searched for again

`buspirate-rs232`, two layers, has 30 nets of two or three pads between two SOIC drivers,
their capacitors and three headers. 0.0.30 routed it with 5 vias and 294 to 300 mm for
the 10 (9 in one seed) and 281 to 283 mm of cadlab's router, 283 to 289 mm outside the
pads for 272 to 274. Net by net, in the first order: 7.1 mm are in three nets that have
no via where cadlab's router has one, and the rest in the runs of 2 to 5 mm from a
driver's pad to the capacitor beside it, 0.2 to 0.4 mm each.

*The short runs.* A search ends at a pad's centre. A capacitor's pad is 0.25 mm off the
line of the driver's, and the run stepped aside for it outside the pad, where cadlab's
router, which ends a track on the first cell of its grid that is on the pad, goes
straight on. `cleanup::aim_at_pads` aims each run's end at a point of the pad's box on
one of the eight directions from its centre, half a track's width from the box's edge at
least, and from there to the centre, which is copper on the pad's own and which a host
leaves out; kept where that leaves less of the run outside the pads and the world allows
it. Outside the pads, over the four orders: 279.9, 281.3, 281.3 and 284.5 mm for 283.2,
286.7, 284.8 and 289.4.

*The three nets.* Each goes between two pads of a driver and round to its own from
behind, 2.7 to 3.3 mm longer than the way with a via that cadlab's router has. They are
made by the pass for fewer vias at the end of the negotiation, whose rule is that
router's (a tenth longer and eight steps for each via less), and kept by the two after
it, which allow 5 mm a via. Tried on the rules, at 40 million nodes over three orders:

| the rules for a via less | `buspirate-rs232`: vias, track outside the pads (60 s, medians) | `buspirate5-rev10` vias | `lumenpnp-mobo` vias | `sweep-v2.2` vias | `tinytapeout-demo` open |
|---|---|---|---|---|---|
| as released | 5, 281.3 mm | 316, 317, 291 | 337, 335, 325 | 22, 18, 20 | 3, 7, 4 |
| a tenth longer and no more, 1.5 mm a via in the passes after | 5, 276.1 mm | 321, 331, 333 | 367, 341, 347 | 25, 22, 23 | 4, 4, 10 |
| a tenth longer and no more, a tenth of the net and 1 mm at least after | 6, 278.1 mm | 321, 325, 330 | 363, 335, 344 | 24, 24, 22 | 4, 5, 10 |

Not kept: what the small board gains, the boards whose vias are level with cadlab's
router's or over lose. What is kept is in the run for shorter ways alone, which takes
each run of tracks out and searches for it again: a via cost it three times what it
costs while routing, 3 mm, at which each of the three nets costs as much with its via as
without; it costs twice now, 2 mm, which is what the negotiation pays for one at a pitch
of 0.2 mm.

| a via in a run searched for again | `buspirate-rs232` at 60 s, four orders: vias; track; outside the pads | `lumenpnp-mobo` | `buspirate5-rev10` | `tinytapeout-demo` |
|---|---|---|---|---|
| 3 mm | 5, 5, 5, 4; 288.8 to 293.4 mm; 279.9 to 284.5 mm | 333 vias, 5334 mm | 308 vias, 2985 mm | 279 vias, 5066 mm |
| 2 mm, as released | 6, 8, 8, 8; 282.5 to 285.7 mm; 273.6 to 276.8 mm | 334 vias, 5326 mm | 306.5 vias, 2977 mm | 284.5 vias, 5038 mm |

The three larger boards are four orders of each at 60 s, one run of each price at a time
side by side, medians; the connections open are the same (none, none, and one in one
order of `tinytapeout-demo`). At 40 million nodes over three orders: `lumenpnp-mobo` 343,
337 and 328 vias for 337, 335 and 325, `buspirate5-rev10` 317, 315 and 297 for 316, 317
and 291, `sweep-v2.2` 23, 20 and 20 for 22, 18 and 20, and no connection more or less on
any of the five boards measured. `tinytapeout-demo`'s vias are worse for it, 5.5 in the
median, on a board whose vias were over those of cadlab's router already.

### Where the time of a run that has every connection goes

`buspirate5-rev10`, four layers, has every connection after 3 s of its first run and
ends after 25 s; `tinytapeout-demo` after 24 s and 53 s. By the clock of each stage
(prints that are in no release), in one order of each:

| stage | `buspirate5-rev10` | `tinytapeout-demo` |
|---|---|---|
| the negotiation and the passes | 2.5 s | 22.9 s |
| the first run's cleanup: each net again, the pads cut off joined again, the searches for fewer vias | 0.5, 1.6 and 3.0 s (398 vias to 385) | 1.5, 2.4 and 6.3 s (350 vias to 331) |
| the run for shorter ways: each net again, the shorter runs | 1.1 and 2.6 s | 2.9 and 5.0 s |
| the host's fills measured again, in every call | 4.3 s over six calls | 3.4 s over three |
| the host between the calls: its check, its fills, the pads' necks | 3.7 s | 3.9 s |
| the step for the spokes, three tries and one | 7 s | 4.5 s |

The fills were the one stage that makes nothing. `Zones::fills` are measured against the
board's copper each time a board is given, and of the 4.3 s, 3.6 s went on making the
parts that a fill of a hundred thousand edges is measured by. `Zones::fills_clear_of` is
the host's word for what its fills were made around; a fill is measured against the rest
alone, and has no parts made if nothing is near it to measure. With it, and the routes
of an earlier run and the fills that are fixed copper counted as what the host filled
around: 1.6 s and 0.2 s, the same tracks and vias to the nanometre on both boards, and
`buspirate-flash-sop` in 0.6 s for 1.1.

### corne-cherry's time, and what was tried on it and not kept

Its first run's negotiation is 150 rounds and 24 s. The first sixteen are 9.7 s and
58 million nodes; cadlab's router's first fourteen are 7.7 s and 56 million: a round
costs the two the same. What differs is that cadlab's router has 18 points shared after
its fourteenth round and ends after 33, at a price for a shared point that rises to a
thousand steps, and this one has some 300 after as many at a price stopped at thirty,
and 135 rounds more of 0.1 s each for the last hundred points. The passes then begin
with 141 of the 152 nets whole and leave 128 to 136 so, and the settling has 3 s for
each of five negotiations for a connection, to the run's deadline.

- *The price rising again once the rounds have not gained for three*: 100 rounds for
  150, 500 vias and 8515 mm for 419 and 7466 mm in the first order, four connections
  open for one.
- *What a round adds to the history of a shared point growing with the rounds*, by a
  tenth and by three tenths of itself a round: 107 and 73 rounds, 18 s both, 458 and
  454 vias.
- *Forty rounds at the first pitch, as at twice the pitch*: 14, 2, 1 and 7 connections
  open over four orders for 1, 0, 1 and 1, in 55 s each for 55, 34, 53 and 55.

## PCB routing with a pad's last spoke kept

What was changed after [the release below](#pcb-routing-with-the-nets-of-a-row-as-narrow-as-its-pitch-asks),
0.0.29, with the same aim: level with cadlab's own router or ahead of it on every measure
on every board of its corpus ([decisions](architecture.md#decisions)). It is still not
met. Measured on 2026-11-23 through cadlab's adapter and its `route_bench`, with a
checkout of this crate before it was released, and with prints and switches in both
routers that are in no release. What is below is at budgets of nodes, on one thread, which
the load of the machine does not move; the table beside cadlab's router at 60 s, with the
crate from crates.io, is in cadlab's `docs/ROUTER.md` and its decision D126.

### A net routed again was kept across another net's track

0.0.25 routes each net again where the others are, by the negotiation's search, and keeps
what it finds if the world says that all of it is free. It asked that by counting: the
tracks that `NetCopper::free_in` keeps of the new route, against the tracks the route has.
`free_in` cuts a track where something is in its way and keeps the free steps on either
side, and joins what is in line: a track cut in two beside two joined into one is as many
tracks as before. `tinytapeout-demo`, at 150 million nodes in the eighth of eight orders:
a track of `ui_in7`, 3.4 mm straight down to its pad of the QFN, across a track of the
ground and one of +3V3, in this crate's own result. cadlab's check then takes out every
piece of copper that touches such a track: 120, 107 and 323 tracks and vias in three of
the eight orders, pieces of the ground and of +3V3 among them, which the runs after the
first join up again. At 60 s, 23 to 228 in two orders of eight. Now every track and every
via of the route is asked of the world (`NetCopper::all_free`), and the check takes
nothing out in any of the eight orders.

### The last spoke of a pad

`RouteOptions::spoke_cost` is a price, and 0.0.29, whose nets of a header are narrow
enough to pass between its pins, left one pad of `tinytapeout-demo` with no spoke in one
order of sixteen at 60 s: pin 5 of J14, of the ground, in a row of pins along which a
track of `ui_in1` ran on one side and one of `ui_in0` on the other. Two things made it,
both found by counting the pad's necks after each of cadlab's runs: its run for shorter
ways, which takes the routes back and looks for a shorter way for each, laid the second
of the two tracks (the pad had two necks before it and none after), and its step that
moves routes out of the rooms of spokes took the pad's last for the second of another
pad's and was kept by the count of the pads short of two. With the fix above alone, the
pad has no spoke at 150 million nodes in the fourth of eight orders, the same way.

`RouteOptions::kept_spokes` keeps a pad's last. Of the spokes that the host's fill makes
for a pad on a layer, those with nothing of another net in their room are free: the
room is the spoke's own copper from where it leaves the pad's box and a clearance on, and
a track or a via stands in it if it is closer to that than the clearance between the two
nets. The board's own copper, the routes of an earlier run that stay where they are among
it, takes a room for good. When a route placed in the world leaves a pad no more free
rooms than it keeps, each of those is put into the world as copper of the pad's net,
which the spoke will be: every search and every step that asks the world then keeps the
clearance from it, and the pad's own net goes through. A route taken out of another room
frees the pad, and the wall goes. The directions that the fill falls back to
(`Zones::spare_spokes`) count for a pad that the board leaves none of the first, and for
no other: a track along the row takes the spoke beside it and both spare ones on its side
at once, and J14's pin, counted with those, had three rooms free and then none.

Three things about when, each measured (below): there is no wall until the nets are
routed a net at a time, so that what a negotiation ends with and what an earlier run left
are put into the world as they are; a room that a route is taken out of is walled only
once something is placed again, so that a route lifted for a search has its way back; and
cadlab has it in its run for shorter ways and in its step for the spokes, and in no run
that routes.

`tinytapeout-demo` at 150 million nodes, eight orders, cadlab's adapter of D126 behind
each but the first:

| | open | vias, median | track, median | pads short of two spokes | pads with none | orders in which the check took copper out |
|---|---|---|---|---|---|---|
| 0.0.29 | 0 in each | 288.5 | 5016 mm | 7, 8, 6, 10, 5, 5, 4, 4 | 0 | 3 (120, 107 and 323 items) |
| the fix of the routes kept across a track alone | 0, 1, 0, 0, 0, 0, 0, 0 | 292.5 | 5042 mm | 7, 9, 6, 7, 5, 5, 5, 5 | 1 in the fourth order | 0 |
| with a pad's last spoke kept, as released | 0, 1, 0, 0, 0, 0, 0, 0 | 292.5 | 5050 mm | 6, 8, 7, 7, 5, 5, 5, 4 | 0 | 0 |

The connection open in the second order is of a run that the budget cut at 150 million,
where 0.0.29 ended at 148.7 million with none. The nine boards that 40 million nodes cut,
three orders, and the seven at 150 million, two orders, are in cadlab's D126: the
connections open are those of 0.0.29 on every board but `tinytapeout-demo`.

### What was tried and not kept

Each on `tinytapeout-demo` at 150 million nodes over the eight orders unless said.

- *The walls from the start of every run, the first among them*, moved as the routes
  are placed: 0, 0, 0, 0, 0, 5, 0 and 0 connections open, 303.5 vias and 5029 mm, 36 pads
  short in all for 49. A negotiation prices the rooms and counts none, and each of two
  nets takes one of a pad's two: the second is cut where it is handed to the passes, and
  the first run of the first order had 343 rip-ups and 38 nets moved aside for 44 and 6.
- *The choice made before anything is routed, for a pad with two free rooms*: the room
  nearer a pad of another net given up, the other walled from the start. No connection
  open, 23 pads short in all, and 316 vias and 5235 mm for 292.5 and 5042 mm.
- *The walls put up once the negotiation's routes are in the world, in every run*: no
  connection open, 298.5 vias, 5032 mm, 39 pads short. `glasgow-revD1` at 40 million
  nodes over three orders: 178, 136 and 151 connections open for 145, 136 and 130.
- *In the runs after the first, those that route among them*: 289 vias and 5051 mm, 46
  pads short. `lumenpnp-mobo` at 150 million nodes: 341 vias for 331 in one order of two,
  a run that moves routes with three nets fewer whole, and the run for shorter ways
  83.8 mm and 5 vias less after it for 142.2 mm and 11.
- *The walls where the board's own copper puts them, from when the world is made*: in a
  run that is given routes, the routes that are not its to move are the board's copper,
  and a route that it is given in the last room such copper leaves a pad was cut as it
  was taken back. `lumenpnp-mobo`'s run for shorter ways came back with a net open and
  was not kept: 351 vias for 331.
- *The room that a route leaves walled at once*: a route lifted for a search and laid
  back where it was is put back by what the world says is free of it, and was cut at the
  room it had stood in.

## PCB routing with the nets of a row as narrow as its pitch asks

What was changed after [the release below](#pcb-routing-with-a-net-that-has-no-way-told-once),
0.0.27, with the same aim: level with cadlab's own router or ahead of it on every measure
on every board of its corpus ([decisions](architecture.md#decisions)). It is still not
met. Measured on 2026-11-23 through cadlab's adapter and its `route_bench`, three threads
for the negotiation, four runs at a time on a machine of 64 threads, with a checkout of
this crate before it was released, and with prints and switches in both routers that are
in no release. The table beside cadlab's router with the crate from crates.io is in
cadlab's `docs/ROUTER.md` and its decision D125.

### The first round of both routers, connection by connection

Both routers were made to print each connection of their first round, in which every net
is routed once with next to nothing priced: its net, the two pads, the layers, the vias
and the length. Joined by net and pair of pads, `tinytapeout-demo`, four layers, with
this crate's negotiation at cadlab's pitch of 0.2 mm (it has twice that by default):
458 connections, 240 vias, 5055 mm and 3514 points shared here, 366 connections, 184
vias, 4494 mm and 2007 points there.

| | connections | vias, here against cadlab's | track, mm |
|---|---|---|---|
| nets with a plane or a pour, whose connections are not the same | 200 against 108 | 27 against 11 | 769 against 419 |
| other nets whose pairs of pads differ | 32 | 23 against 16 | 434 against 399 |
| the same pair, no via in either | 84 | 0 | 631 against 553 |
| the same pair, as many vias | 89 | 113 | 1915 against 1874 |
| the same pair, more vias here | 37 | 66 against 15 | 893 against 867 |
| the same pair, fewer vias here | 16 | 11 against 29 | 413 against 383 |

- *The ground*, planned here on its inner planes alone: 124 connections, 27 vias and
  324 mm, where cadlab's router takes the pads that the pour of their own layer joins as
  joined and has 32 connections, no via and 32 mm. Planned on every pour, as there, the
  first round has 108 connections of such nets with 15 vias and 465 mm; the board ends
  worse for it (below).
- *The room of a spoke*, which costs a track here and nothing there: with no price for
  it the connections with more vias here are 27 with 45 for 11, where they were 37 with
  66 for 15. The board ends with more pads short of their spokes without it (below).
- *The order*, strides through the nets here and the shortest first there: 32 with 55
  for 12.
- *The pairs of pads*, a spanning tree by the distance along the axes here and as the
  crow flies there: 8 nets, 7 vias and 35 mm.
- *The ends*: a route here ends at the middle of a pad, by a track from the grid's point
  to it, and there on the first point of the grid that is on the pad's copper. The 84
  connections with no via in either are 78 mm longer here, of which 67 mm are those
  tracks; cadlab's routes end 116 mm short of the pads' middles in all.
- With all of those taken away (no price for the room of a spoke, the shortest first,
  every pour planned on, a via of a net with a plane at ten steps): 194 vias, 4746 mm and
  2765 points shared, against 184, 4494 mm and 2007. The connections with more vias here
  less those with fewer are then four vias.
- *The widths.* What was left was looked at a connection at a time: `uio3` from one pin
  of a header to another, 11.4 mm here round the header and 6.7 mm there between two
  rows of its pins. A track of 0.22 mm there is 0.245 mm from a pin of the ground, whose
  class asks 0.25 mm. cadlab's router routes that net at 0.185 mm: 100 of the board's 137
  nets at 0.185 mm for 0.22 mm, 22 at 0.235 mm for 0.25 mm and 9 at 0.16 mm, where this
  crate routed 95 of them at 0.22 mm. Both routers make a net narrower for a row of pads
  of fine pitch, by one rule each: there the pitch of two pads less the net's clearance
  and what a grid's points miss between them; here the widest at which two tracks out of
  two pads side by side clear each other by the clearance of the area that both are in,
  which in the courtyard of this board's QFN is 0.16 mm for the board's 0.2 mm. So 80
  tracks of 0.22 mm left that courtyard 0.18 mm apart and had to part at its edge.

The same join on the other boards, at this crate's own pitch (twice cadlab's on more
than two layers): `buspirate5-rev10` 269 vias and 3191 mm for 115 and 2812 mm, of which
its nets with pours 130 vias for 17 (the ground 168 connections and 97 vias for 7 and
none) and 22 connections of the same pair with 44 vias for 1; `glasgow-revC3` 284 vias
and 3874 mm for 313 and 3528 mm, its nets with pours 183 vias for 166 and 897 mm for
715; `glasgow-revD1` 1040 vias and 10 347 mm for 743 and 8778 mm, of which its nets with
pours 378 vias for 133 and 1015 mm for 615 (the ground 583 connections and 339 vias for
225 and 100), 484 connections of nets whose pairs differ 3974 mm for 3516, 16
connections that cadlab's router does not make at all 338 mm, 482 of the same pair with
no via 1024 mm for 839 and 185 with as many vias 2696 mm for 2557.

And the rounds after the first, on `glasgow-revD1`: cadlab's router has five in its share
of a minute, and goes from 743 vias and 8778 mm to 939 and 9123 mm with 918 points shared;
this crate has 28, and goes from 1040 and 10 347 mm to 1264 and 11 842 mm with 190. What
it has over cadlab's router on that board's track is there after its first round.

### What was changed

1. *In a rule area with a clearance of its own, a net of a row is no wider than the
   row's pitch has room for two of*
   (`pcb::neck`, in a run with a budget or a deadline, as the rule it is added to): the
   pitch of the pad and the nearest pad of another net beside it, less the net's own
   clearance, the board's and not an area's, less what a grid's points miss between two
   tracks a pitch apart and the rounding of the rules' distances, in steps of 5 µm and
   never under the least width. For a pad on one layer with no room for a track of the
   net's width between it and that pad. It is cadlab's rule (`fanout::necks`), and gives
   its widths on `tinytapeout-demo`: 53 nets at 0.185 mm, 22 at 0.235 mm, 9 at 0.16 mm.
   The other eleven boards have the widths they had. 0.0.28 had the rule for every row,
   in an area or not, and 0.0.29 has it for the rows in an area alone (below).
2. *The lattice has the pitch of the tracks that a row made narrower*, half their
   width and their clearance, where that is tighter than the pitch of the nets' own
   widths. `tinytapeout-demo`: 0.1925 mm for 0.2 mm. Not in a run with a budget of nodes.

`tinytapeout-demo`, four orders at 60 s, medians:

| | open | vias | track, mm | track outside the pads, mm |
|---|---|---|---|---|
| 0.0.27 | 0, 1, 1, 0 | 300 | 5376 | 5272 |
| the nets at the widths of cadlab's router, given as their classes' | 0, 1, 0, 0 | 270 | 4967 | 4889 |
| 1 alone | 0, 3, 0, 0 | 296 | 5140 | 5061 |
| 1 and 2 | 0, 1, 0, 0 | 268.5 | 4961 | 4882 |
| 1 and 2, four other orders | 0, 2, 1, 4 | 295 | 5162 | 5087 |

cadlab's router has 273 vias, 4894 mm and 4672 mm outside the pads over eight seeds. At
this crate's own pitch the rounds settle sooner with the narrower nets: 41 points shared
after the twelfth round for 123, with 368 vias for 370 and 5481 mm for 5658.

Budgets of nodes, one thread, 0.0.29: at 40 million over three orders `tinytapeout-demo`
has 3, 7 and 10 connections open for 8, 4 and 13, and the eight other boards that a
budget cuts what they had, to the via (`corne-cherry` 36, 28 and 22); at 150 million
over two orders `tinytapeout-demo` none with 269 and 331 vias for none with 300 and 298,
and `corne-cherry` 8 and 0 as it had.

### Only in an area: 0.0.29

0.0.28 had the rule for every row. Without an area this crate's own rule for the pads
beside a pad is told by the board's clearance already, and cadlab's rule differs from it
by what a grid's points miss: `corne-cherry`, two layers, had its 90 nets at 0.2 mm for
0.21 mm on a lattice of 0.195 mm for 0.22 mm, and `buspirate5-rev10` one net at
0.235 mm for 0.244 mm. `corne-cherry` at 60 s, four orders, four measurements of 0.0.28
before and after it was released: 1, 0, 4 and 4 connections open, the same again, 0, 0, 4
and 8, and 1, 0, 4 and 4 beside cadlab's router, where 0.0.27 had 1, 0, 1 and 1 beside it
and cadlab's router has 2 to 4. At 40 million nodes it had 27, 21 and 44 open for 36, 28
and 22, and at 150 million 1 and 0 for 8 and 0. So the rule is for the rows in an area
with a clearance of its own under the net's, which is where the two rules part by more
than a grid's points: with 0.0.29 `corne-cherry` and `buspirate5-rev10` are routed as by
0.0.27, to the via at a budget of nodes, and `tinytapeout-demo` as by 0.0.28.

### Tried and not kept

- *The lattice at the pitch of every net made narrower*, not those of rows alone:
  `glasgow-revD1`, whose 160 nets are narrower for the balls of its array, 12, 24, 23 and
  28 connections open over four orders at 60 s for 11, 28, 7 and 9, and 18 to 24 pads
  short of two spokes for 8 to 22.
- *The finer lattice in a run with a budget of nodes*: 40 million, three orders,
  `corne-cherry` 110 open for 86 and `tinytapeout-demo` 36 for 25.
- *A net with a plane planned on its pours of the outer layers too, with the pads cut
  off told again when the rounds end at their most*: the release before the last tried
  the first half. `tinytapeout-demo` 281 vias and 5210 mm over four orders for 281.5 and
  5043 mm; `glasgow-revD1` 81, 66, 66 and 77 connections open for 7 to 36.
- *No price for the room of a spoke in the rounds*: `buspirate5-rev10` 307.5 vias as with
  it and 16, 16, 17 and 11 pads short of two spokes for 6 to 12; `tinytapeout-demo` 281
  vias and 5105 mm for 281.5 and 5043 mm.
- *The shortest nets first in the first round*, *the spanning tree as the crow flies*
  and *a route's end on any point of its pad's copper, at no cost*: `tinytapeout-demo`
  281, 290 and 297.5 vias for 268.5 in the same four orders. The second on
  `glasgow-revD1`'s first round: 10 324 mm for 10 347.
- *A via of a net planned on outer pours at twenty steps of twice the pitch*, as a board
  of two layers has it, and *eight times a via's price in the stubs that take its pads
  back to their fills*: `buspirate5-rev10` 302.5 and 306.5 vias for 307.5.
- *One time of telling the pads cut off their fills, or none*, for three:
  `tinytapeout-demo` 7, 1, 10 and 1 connections open, and 13, 1, 10 and 2, for 0, 1, 0
  and 0.
- *The rounds ended when the fewest points shared are no fewer than three quarters of
  what they were eight rounds before* (and half, twelve rounds before, at the first
  pitch): `tinytapeout-demo` 274.5 vias and 4999 mm for 281.5 and 5043 mm with 2
  connections open over four orders for 1, `corne-cherry` 2, 1, 2 and 2 open in 54, 35,
  54 and 55 s for 0, 0, 4 and 8 in 54, 54, 54 and 59 s. Within what the same code moves
  from one measurement to the next.

### What this does not show

More than four runs of anything but `tinytapeout-demo`. Where `buspirate5-rev10`'s nets
with pours on its outer layers get their vias: 43 to 45 for cadlab's router's 12 to 14 on
one of them, with as much track; not in the rounds and not in the stubs to the fills, by
the two prices tried. `glasgow-revD1`'s track, which is there after its first round on a
grid of twice the pitch. `corne-cherry`'s time: its first run's rounds are 24 s, of which
the fifty before the pads cut off are told for the first time are 17 s, and the runs go
on for as long as a connection is open.

## PCB routing with a net that has no way told once

What was changed after [the release below](#pcb-routing-with-each-net-routed-again-where-the-others-are),
0.0.25, with the same aim: level with cadlab's own router or ahead of it on every measure
on every board of its corpus ([decisions](architecture.md#decisions)). It is still not
met. Measured on 2026-11-23 through cadlab's adapter and its `route_bench`, three threads
for the negotiation, on a machine of 64 threads at a load of 3 to 15, with a checkout of
this crate before it was released, and with prints in it that are in no release. The
table beside cadlab's router with 0.0.26 from crates.io is in cadlab's `docs/ROUTER.md`
and its decision D124.

### What a connection with no way cost

`cynthion`, four layers, has eight nets of two pads at its four net ties, each with a pad
that no track leaves at the board's clearance from the pads beside it. One run of 60 s,
the first run of the adapter, whose deadline is at 42 s:

| | 0.0.25 | 0.0.26 |
|---|---|---|
| the first pass on the finer lattice | 12.5 s | 2.5 s |
| the second | 9.3 s | none of its nets has a turn |
| one of the eight nets, in each of the two passes | 6 523 250 nodes, 9.5 s | 238 059 nodes, 0.24 s, once |
| the first run ends at | 40 to 42 s, its deadline | 20.7 s |
| pads short of two spokes as the adapter ends, four orders | 5, 5, 1 and 1, two with none in two orders | 1, 1, 1 and 1 |

- **The search from the pad that routes shut in went over the whole board.** A search
  that may cross other nets' routes is from the terminal that a look found shut in with
  nothing crossed, first within a window and then anywhere. Where the other end is the
  one that nothing reaches, shut in by pads, that search crosses every route of the
  board before it says so. It now has 200 000 nodes once it leaves its window, then a
  look as long has from the other end, and only then all that it may expand
  (`detail::DEEP_LOOK`).
- **The search on the board with no route said nothing of them.** It kept what it
  reached and went on from the other terminal with an eighth of its nodes, 500 257 on
  this board, ran out, and a search that runs out says nothing: the eight nets had no
  floor, and a turn in every pass. For a net of two terminals it is now the search for
  the net whole, which says that it has a way, that it has none, or that it ran out.
- **A search that may cross every route and finds none has said it for the run.** On the
  finest lattice of the run, for a net of two terminals whole, with no net locked: what
  it could not cross is the board's own (`router::shut_in`). Such a net has no more
  turns, in no pass and in no chain of the settling.
- **The run tells them** (`Routed::apart`), for the host's later runs to leave them
  out, and their open connections say what shuts the pad in where the board's own does
  (`Cause::Enclosed`, whichever end of the connection it is), which
  `Cause::Apart` had hidden: cadlab's report of the eight was "the board keeps U1.14
  apart from R59.2" in the first build of this, where its own router says "no legal way
  out of R59.2 for a 0.127mm track: pad R59.1".

### What telling the needless vias cost

`pcb::needless` and `pcb::needless_between`, which cadlab's adapter asks after its runs, on
`cynthion` as routed: 2.2 s and 1.9 s, of which 1.7 s and 1.5 s for the ground alone, 597
tracks and vias against 184 pieces of fill. Not the measuring: each piece was cut into
parts of few edges the first time a point was asked of it, 0.1 to 0.5 s for a piece of
a plane, 15 such cuts in a run of 40 s, for the 1600 points that the question asks. A
piece now answers its first 4096 points by its edges (`planes::DIRECT_HOLDS`), and has
its parts when it has been asked more: 0.34 s and 0.28 s for the two, and no piece cut in
that run. At 40 million nodes on one thread `cynthion`, `glasgow-revC3` and
`tinytapeout-demo` have the same connections, vias and track to the tenth of a
millimetre as before it, in as long.

### Each net routed again before the fills are joined again

In a run that its deadline ends, cleanup has 8 % of what the negotiation left, and the
joining of what the routes cut off their fills came first: `glasgow-revC3` had 0.35 s of
it left for [each net routed again](#pcb-routing-with-each-net-routed-again-where-the-others-are),
89 nets of 369, where all of them take 0.7 s. It is first now, with half of cleanup's
time and not with less than a second (`router::AGAIN_LEAST`), and the stubs to the fills
are for the routes as they stay. `glasgow-revC3`, four orders at 60 s, before and after:

| | open | vias | track |
|---|---|---|---|
| before | 12, 6, 10, 8 | 420, 398, 397, 410 | 3676, 3727, 3729, 3635 mm |
| after | 11, 6, 9, 8 | 418, 398, 389, 409 | 3589, 3729, 3668, 3586 mm |

`glasgow-revD1`, six layers, has 0.8 s for cleanup after a negotiation of 30 s, and is
as it was: asking the grid of its board takes more than that.

Not in a run with a budget of nodes (0.0.27). There the joining makes connections with
the work that is left, each net routed again makes none, and 0.0.26, which had it first
there too, left more open at 40 million nodes on one thread, the measure that does not
move with the machine and that was not taken before that release:

| 40 million nodes, three orders | 0.0.25 | 0.0.26 | 0.0.27 |
|---|---|---|---|
| `corne-cherry` | 36, 28, 22 | 48, 52, 52 | 36, 28, 22 |
| `tinytapeout-demo` | 8, 4, 13 | 15, 11, 13 | 8, 4, 13 |
| `glasgow-revC3` | 12, 15, 12 | 11, 12, 17 | 12, 15, 12 |
| `glasgow-revD1` | 145, 136, 130 | 146, 136, 130 | 145, 136, 130 |
| the nine boards that the budget cuts | 594 | 676 | 594 |

With half of the work for it and the joining after, `corne-cherry` has 38 in the order
given and `tinytapeout-demo` 10; with a quarter 38 and 9; with a tenth 38 and 8. At 150
million nodes, two orders, 0.0.26 had `corne-cherry` at 12 and 0 for 8 and 0.

### Tried and not kept

- *Two or three nets moved at once*, which the release below named: a net with vias is
  looked for a way with fewer across the claims of the others at a price, the one or two
  nets it crosses are taken out, the net is laid and they are routed again round it,
  kept if all are whole and have fewer vias together. `tinytapeout-demo`, the first run:
  of 92 nets with vias, 17 have such a way across one or two nets at 8 steps a claimed
  point; for 2 the world does not allow it, for 11 a net crossed has no way round it, 2
  are routed whole, and 1 is kept, for one via of 359. At 40 and at 150 steps 7 and 6
  have such a way and none is kept.
- *More of the time for cleanup*, 12 % for 8 %: `glasgow-revC3` 3623 mm for 3628 over
  four orders. And 30 % and 50 % on `glasgow-revD1`: each net routed again begins with
  the nets of most vias, 400 000 nodes each, and has 2 and 18 nets in 1 s and 2.2 s; the
  board 14 and 13 connections open for 8, with 10 664 and 10 583 mm for 10 777.

### What this does not show

More than four runs of anything, and one of most. `corne-cherry`'s time, which is its
negotiation at the first pitch: 151 rounds in 25 s, with 5389 points shared after the
first, 100 after the thirtieth, 36 to 60 from the sixty-first to the seventy-third, and
one from the hundred and thirty-eighth on. `glasgow-revD1`'s track, above. The vias and
the track of `tinytapeout-demo`, as in the release below.

## PCB routing with each net routed again where the others are

What was changed after [the release below](#pcb-routing-with-a-pour-on-the-pads-own-layer),
0.0.24, with the same aim: level with cadlab's own router or ahead of it on every measure
on every board of its corpus ([decisions](architecture.md#decisions)). Measured on
2026-11-23 through cadlab's adapter and its `route_bench`, on a machine of 64 threads at a
load of 2 to 7, with a checkout of this crate before it was released, and
[beside cadlab's router](#beside-cadlabs-router-0025) with 0.0.25 from crates.io.

### Where the two negotiations part

Counted with prints and switches in both routers that are in neither release.
`tinytapeout-demo`, four layers, after the first round, in which each net is routed once
with next to nothing priced, both at the same pitch of 0.2 mm:

| | cadlab's router | `guide` 0.0.24 | `guide`, a plane and a spoke's room at no price |
|---|---|---|---|
| vias | 184 | 244 | 221 |
| track | 4494 mm | 5153 mm | 5014 mm |
| points shared | 2007 | 5429 | |
| the ground, by net | 32 connections, no via, 32 mm | 124 connections, 27 vias, 318 mm | |
| the other nets | 173 vias, 4076 mm | 216 vias, 4390 mm | 194 vias, 4260 mm |

So a third of what the rounds have to settle is there before the first of them ends, and
it is not the rounds that put it there. The ground is planned on its planes alone, each
pad on the top layer with a stub of its own, where cadlab's router takes the pads that the
pour on their own layer joins as joined; and the other nets pay twice their length inside
a plane, which on a board with a plane on two of its four layers halves what a via costs
them beside a way round.

What the rounds route again was not it. With every net near a shared point routed again
whole, as cadlab's router has it, with the nets of a round one at a time and not eight,
and with both, the vias of `tinytapeout-demo` at twice the pitch went from 243 to 247 after
the first round to 403 to 406 after the ninth, for 247 and 410 with neither.

### What was changed

- **A plane of another net costs a track nothing in the negotiation.** The passes that
  route a net at a time pay for it as they did
  ([`RouteOptions::plane_cost`](../src/pcb/mod.rs)). At 150 million nodes, four orders,
  medians, with the price and without it:

  | board | open, with | open, without | vias | track |
  |---|---|---|---|---|
  | `tinytapeout-demo` | 0, 1, 0, 4 | 0, 0, 0, 0 | 324 for 299 | 5133 for 5071 mm |
  | `buspirate5-rev10` | 0, 0, 0, 0 | 0, 0, 0, 0 | 322 for 308.5 | 3068 for 3017 mm |
  | `lumenpnp-mobo` | 0, 0, 0, 0 | 0, 0, 0, 0 | 331.5 for 333 | 5483 for 5407 mm |
  | `cynthion` | 8, 8, 8, 8 | 8, 8, 8, 8 | 418 for 416.5 | 3088 for 3072 mm |
  | `glasgow-revC3` | 6, 5, 5, 5 | 6, 6, 6, 4 | 406.5 for 395 | 3699 for 3613 mm |

- **Forty rounds at most at twice the pitch**, as cadlab's router has, where there were a
  hundred, and a negotiation that ends at its last round is not begun again in another
  order. `tinytapeout-demo` has 44 points shared after its fortieth round and 16 to 28
  after some 150, with 349 to 388 vias as the rounds end for 394 to 458: at 150 million
  nodes over six orders, 305 vias and 5037 mm with forty for 311.5 and 5412 mm with a
  hundred. At the first pitch, where the price of a shared point stops at thirty steps,
  the hundred stay: `corne-cherry` had 11, 2, 3 and 9 connections open in four orders at
  60 s with forty for 4, 0, 0 and 0.
- **Each net routed again where the others are**, once every route is where it is
  (`whole::again`): the negotiation's search, over a grid of which every place was asked
  of the board once, in place of the searches of cleanup, which cost many times as much a
  net. The routes are no longer the negotiation's by then, so each net claims on the grid
  what its copper keeps other nets' away from wherever it lies, by as much more for copper
  off the grid as a track on it may pass nearer between two of the grid's points; a net
  gives up its claims while it is routed where nothing is claimed; and what the search
  finds is kept only if the world, which has the others' copper as it is, allows all of
  it. The first time through the nets is for fewer vias alone, at what cleanup lets a via
  less cost in track, and the times after for a way as short with as many as well. No
  layer has a direction of its own in it: with those, `buspirate-rs232` kept 7, 6, 6 and
  5 vias in four orders where cleanup alone leaves 5, 5, 5 and 4. On the negotiation's
  grid in a run with a budget, 1 to 4 s a board; at the pitch in a run for shorter ways
  ([`RouteOptions::shorten`](../src/pcb/mod.rs)).
- **A run of tracks laid round the corners of what is beside it**, off the lattice, in a
  run for shorter ways (`cleanup::round_corners`): cadlab's gridless pass. The corners are
  those of the octagon round each corner of a pad, a track's end, a via or a hole, whose
  sides are as far from it as the track's centre has to stay; each stretch of a run is the
  shortest way through the corners near it by tracks in the eight directions that the
  world allows, and is laid if it is shorter.

  `tinytapeout-demo` at 150 million nodes, six orders, medians, with the two changes
  above in all four:

  | | vias | track |
  |---|---|---|
  | round the corners alone | 315 | 5200 mm |
  | routed again alone | 308.5 | 5102 mm |
  | both | 305 | 5037 mm |
  | neither, a plane at its price and a hundred rounds: as 0.0.24 | 333 | 5202 mm |

- **A pass on the finer lattice that took no net out is the last.** The next has every
  net free to be taken out again and nothing else. `sweep-v2.2`, whose three open
  connections no router makes, had a pass of 13 s and one of 6 s for them, and has the
  first: its minute ends after 39 to 41 s for 58, with cadlab's adapter no longer
  beginning a run that moves routes after a first run that moved none.

At 40 million nodes, three orders of each of the nine boards that a budget cuts, 594
connections open for 0.0.24's 597: `glasgow-revC3` 39 for 42, and each of the other eight
with as many as it had.

### Beside cadlab's router, 0.0.25

As released, with the crate from crates.io. Four runs of each, one of each at a time side
by side: cadlab's router with seeds 1 to 4 on three threads, `guide` in four orders with
three threads for its negotiation; and four more of each on `tinytapeout-demo`, whose
runs end on the deadline. The machine's load was 2.2 to 4.0. Connections open by run, and
vias and track by the medians:

| board | cadlab's router: open | `guide` 0.0.25: open | vias, `guide` against cadlab's | track |
|---|---|---|---|---|
| `buspirate-flash-sop` | 0, 0, 0, 0 | 0, 0, 0, 0 | 5 against 4 | 440 against 461 mm |
| `buspirate-rs232` | 0, 0, 0, 0 | 0, 0, 0, 0 | 5 against 10 | 297 against 281 mm |
| `lumenpnp-ringlight` | 0, 0, 0, 0 | 0, 0, 0, 0 | 3 against 3 | 167 against 177 mm |
| `nrfmicro` | 0, 0, 0, 0 | 0, 0, 0, 0 | 42.5 against 47.5 | 437 against 450 mm |
| `sweep-v2.2` | 3, 3, 3, 3 | 3, 3, 3, 3 | 20 against 25.5 | 2122 against 2159 mm |
| `corne-cherry` | 3, 3, 2, 4 | 1, 0, 1, 0 | 427.5 against 459 | 7552 against 8103 mm |
| `tinytapeout-demo` | 19, 2, 5, 5, 2, 2, 3, 9 | 0, 0, 0, 0, 1, 0, 0, 7 | 308 against 273 | 5254 against 4894 mm |
| `buspirate5-rev10` | 4, 2, 4, 8 | 0, 0, 0, 0 | 308.5 against 302 | 3017 against 3129 mm |
| `lumenpnp-mobo` | 8, 12, 7, 15 | 0, 0, 0, 0 | 335.5 against 343.5 | 5407 against 5462 mm |
| `cynthion` | 28, 29, 29, 28 | 8, 8, 8, 8 | 422 against 478 | 3125 against 3190 mm |
| `glasgow-revC3` | 36, 31, 35, 31 | 9, 6, 8, 5 | 406 against 416.5 | 3756 against 3478 mm |
| `glasgow-revD1` | 117, 108, 94, 96 | 8, 15, 7, 12 | 1168 against 1089.5 | 10748 against 9117 mm |
| all twelve, the first four runs | 218, 190, 179, 190 | 29, 32, 27, 28 | | |

No DRC error other than unrouted in the 104 runs. By cadlab's count of equal, better and
worse over six measures on each board (open, vias, track, track outside the pads, pads
short of two necks, time): 27 better, 29 equal, 1 with ranges that overlap and 15 worse,
for 24, 29, 2 and 17 with 0.0.24. No longer behind: the time of `sweep-v2.2`, 40 to 42 s
for 58 s where cadlab's router takes 51 to 52 s, and of `buspirate5-rev10`; the track
outside the pads of `lumenpnp-mobo` and `cynthion`. Still behind, and nearer: the vias of
`tinytapeout-demo` (+13 % for +32 %), `glasgow-revD1` (+7 % for +11 %),
`buspirate5-rev10` (+2 % for +9 %) and `buspirate-flash-sop` (5 against 4 for 6); the
track of `tinytapeout-demo` (+7 % for +16 %), `glasgow-revC3` (+8 % for +14 %) and
`buspirate-rs232` (+6 %, as it was). By the nets that both routers complete, the medians
of the pairs of runs: `tinytapeout-demo` +12 % vias and +9 % track for +34 % and +18 %,
`buspirate5-rev10` +8 % and +2 % for +13 % and +4 %, `glasgow-revC3` 13 % fewer vias and
+2 % track, `cynthion` 22 % fewer and 3 % less, `corne-cherry` 10 % fewer and 5 % less.

Worse than with 0.0.24: the track of `glasgow-revD1`, +18 % over cadlab's router's for
+11 %, with 7 to 15 connections open for 22 to 39, and +10 % for +5 % on the nets that
both complete. With no price for a plane in the negotiation more of the nets of its six
layers find a way, and the ways are longer: at 60 s over four orders, that change alone
took it from 34, 28, 25 and 39 open with 10035 mm to 23, 28, 5 and 8 with 10779 mm. And
`cynthion`'s pads short of two spokes, 5, 5, 1 and 1 for 0, 1, 1 and 4, in the two orders
whose first run ends at its deadline with two passes on the finer lattice of 14 s and
11 s for eight nets that have no way; the pads of the twelve boards short of two have
28, 36, 26 and 36 for 36, 36, 41 and 44.

With budgets of nodes, one thread: the nine boards that 40 million cut have 594
connections open over three orders for 597, `glasgow-revC3` 39 for 42 and each of the
others as many as it had. At 150 million, two orders, every board has the connections
open that it had, and `tinytapeout-demo` 300 and 298 vias for 320 and 329.

### Tried and not kept

- *The negotiation at the first pitch on four layers, its price rising to a thousand
  steps* (`tinytapeout-demo`, four orders at 60 s): its share ends it after 25 to 78
  rounds with 42 to 122 points shared, and the board has 6, 2, 0 and 4 open with 344 vias.
- *A net with a plane planned on its pours of the outer layers as well*, as cadlab's
  router has it: the first round has 168 vias where it had 247, and at 150 million nodes
  over six orders the board ends with 337.5 vias, 5442 mm and 5 connections open for 300,
  5071 mm and none.
- *The price of a shared point rising at the first pitch*, again, now that each net is
  routed again afterwards: `corne-cherry` 505.5 vias and 8200 mm over four orders for 432
  and 7556 mm.
- *The pads cut off their fills told at the last round too*, and forty rounds more for
  them: `tinytapeout-demo` 316 vias and 5501 mm at 60 s for 309.5 and 5186 mm.
- *Every net near a shared point routed again whole, and the nets of a round one at a
  time* (above).

### What this does not show

More than four or six runs of anything; other boards than cadlab's twelve. Still behind
cadlab's router after this, by the same runs: the vias that a round of the negotiation at
twice the pitch leaves on `tinytapeout-demo` (299 to 305 at 150 million nodes for the 272.5
of cadlab's router at 60 s, where 0.0.24 had 333), which no net loses by being routed
again alone, as the nets in its way would have to move with it; and the time of the
passes on the finer lattice for connections that have no way, where one of them moved a
net aside (`cynthion`: two passes of 14 s and 11 s for its eight nets at their net ties).

## PCB routing with a pour on the pads' own layer

What was changed after [the release below](#pcb-routing-where-cadlabs-router-was-still-ahead),
0.0.23, with the same aim: level with cadlab's own router or ahead of it on every measure
on every board of its corpus ([decisions](architecture.md#decisions)). It is still not
met. Measured on 2026-11-22 and 23 with 0.0.24 from crates.io through cadlab's adapter and
its `route_bench`, on a machine of 64 threads at a load of 2 to 6.

### The two routers stage by stage

Counted with prints in both that are in neither release, one run of each at 60 s on
three threads. `tinytapeout-demo`, four layers:

| | cadlab's router | `guide` 0.0.23 |
|---|---|---|
| the rounds of the negotiation | 38 in 17 s, 2 points shared at the end | 177 in 23 s at twice the pitch, 12 to 116 shared from the eleventh on |
| as the rounds end | 255 vias, 4686 mm | 458 vias, 5792 mm |
| after the pass for fewer vias | 248 vias, 5013 mm (1.9 s, after its rounds for the pours) | 381 vias, 5660 mm (0.4 s) |
| after what closes the rest | | 412 vias, 5978 mm |
| the result | 244 vias, 4947 mm, 19 open | 344 vias, 5453 mm, none open |

The gap is made in the rounds, where only the wires in another net's way are routed again
and a wire that went under another stays there: by net, in cadlab's dumps of one run of
each, the nets that both routers complete with no via in either are as long or shorter
here on every board, and the nets where this router has more vias carry its extra track
(`tinytapeout-demo`: 43 nets, 144 vias for 66, 2223 mm for 1965). Negotiated at cadlab's
pitch, this board's rounds take 0.8 to 1.8 s for 0.15 s, the share ends them after the
twelfth with 151 points shared, and the minute ends with 8 connections open for none.

`corne-cherry`, two layers, both at the same pitch, by the points shared after each
round: 4792, 2220, 1457, 1020, 712, 471, 325, 280, 237, 193, 141, 60, 34, 18 for cadlab's
router, which ends after 9 s; 4338, 1942, 1373, 866, 638, 413, 370, 310, 289, 172, 160,
157, 174, 125 here, and 66 to 168 for the seventy rounds after, until the share ends
them at 19.7 s. They part at the tenth round, where cadlab's price for a shared point
goes on rising and this one stops at thirty steps, for the copper (tried rising again
in 0.0.23, below, and not kept).

### A via of a net with a pour, on two layers

By the nets with zones and the others, four runs of each at 60 s with 0.0.23: the vias
that `corne-cherry` had over cadlab's router were all those of its two grounds, 140 to
157 against 65 to 71, with half the track (349 to 496 mm against 879 to 928 mm); its
other nets had 349 to 368 against 370 to 416. Both routers leave the pours in as many
pieces; cadlab's joins them by tracks along the pads' own layer, from its pass for fewer
vias, which routes a net again a connection at a time. The pass here routes a net again
whole and keeps it or not: with the time to end and the nets with planes first it took
40 to 50 vias inside the negotiation and left the grounds with 131 to 157 at the end of
the minute. With forty steps for the via of a net with a plane from the first round on,
at the first pitch, and eight times the passes' price in the stubs back to a fill on two
layers, four orders at 60 s:

| `corne-cherry` | 0.0.23 | 0.0.24 | cadlab's router |
|---|---|---|---|
| vias of the two grounds | 130 to 157 | 92 to 111 | 65 to 71 |
| vias of the board | 459 to 519 | 418 to 458 | 441 to 481 |
| track | 7407 to 7520 mm | 7230 to 7659 mm | 8036 to 8203 mm |
| connections open | 0, 2, 0, 2 | 4, 0, 1, 1 and 1, 0, 0, 1 | 3, 3, 2, 4 |

(0.0.24 measured twice; its runs end on the deadline in two orders of four.)
`lumenpnp-ringlight`'s ground has one via and 4.2 to 4.5 mm of track for two and 2.9 mm,
and the board 1, 3, 3 and 3 vias as cadlab's router's. Twenty and thirty steps: 464 and
484 vias with 1 and 4 open, 457 and 459 with 5 and none, over two orders.

### Less outside the pads

`buspirate-rs232`, four orders: 294.6 to 299.6 mm of track for 297.7 to 302.7 mm, and
284 mm outside the pads of its nets for 287 mm; cadlab's router has 281 and 272 mm, with
ten vias for five. Its nets of two pads between a SOIC and its capacitors are 2.1 mm
each for 2.6 mm (cadlab's router: 1.8 mm, a straight track from the one pad's copper to
the other's).

### Beside cadlab's router, 0.0.24

Four runs of each, one of each at a time side by side: cadlab's router with seeds 1 to 4
on three threads, `guide` in four orders with three threads for its negotiation.
Connections open by run, and vias and track by the medians:

| board | cadlab's router: open | `guide` 0.0.24: open | vias, `guide` against cadlab's | track |
|---|---|---|---|---|
| `buspirate-flash-sop` | 0, 0, 0, 0 | 0, 0, 0, 0 | 6 against 4 | 443 against 461 mm |
| `buspirate-rs232` | 0, 0, 0, 0 | 0, 0, 0, 0 | 5 against 10 | 297 against 281 mm |
| `lumenpnp-ringlight` | 0, 0, 0, 0 | 0, 0, 0, 0 | 3 against 3 | 168 against 177 mm |
| `nrfmicro` | 0, 0, 0, 0 | 0, 0, 0, 0 | 42.5 against 47.5 | 440 against 450 mm |
| `sweep-v2.2` | 3, 3, 3, 3 | 3, 3, 3, 3 | 21 against 25.5 | 2137 against 2159 mm |
| `corne-cherry` | 3, 3, 2, 4 | 1, 0, 0, 1 | 430 against 459 | 7541 against 8103 mm |
| `tinytapeout-demo` | 19, 2, 5, 5 | 0, 3, 0, 0 | 358.5 against 272.5 | 5687 against 4913 mm |
| `buspirate5-rev10` | 4, 2, 4, 8 | 0, 0, 0, 0 | 329.5 against 302 | 3078 against 3129 mm |
| `lumenpnp-mobo` | 8, 12, 7, 15 | 0, 0, 0, 0 | 337.5 against 343.5 | 5532 against 5462 mm |
| `cynthion` | 28, 29, 29, 28 | 8, 8, 8, 8 | 427.5 against 478 | 3165 against 3184 mm |
| `glasgow-revC3` | 36, 32, 33, 31 | 8, 7, 8, 7 | 415 against 419.5 | 3949 against 3462 mm |
| `glasgow-revD1` | 111, 110, 94, 102 | 28, 22, 39, 36 | 1200.5 against 1084.5 | 10073 against 9090 mm |
| all twelve | 212, 193, 177, 196 | 48, 43, 58, 55 | | |

No DRC error other than unrouted in the 96 runs. By cadlab's count of equal, better and
worse over six measures on each board (open, vias, track, track outside the pads, pads
short of two necks, time): 24 better, 29 equal, 2 with ranges that overlap and 17 worse,
for 21, 28, 3 and 20 with 0.0.23. No longer behind: the vias of `corne-cherry` and of
`lumenpnp-ringlight`, and the track of `cynthion`, where cadlab's adapter now ends the
runs that move routes after one that has said all there is, and has the time for the
tracks between pads that a pour joins. Still behind: the vias of `tinytapeout-demo`
(+32 %), `glasgow-revD1` (+11 %), `buspirate5-rev10` (+9 %) and `buspirate-flash-sop` (6
against 4, two of them joining the pieces of a pour with no track); the track of
`tinytapeout-demo` (+16 %), `glasgow-revC3` (+14 %), `glasgow-revD1` (+11 %) and
`buspirate-rs232` (+6 %); the time of `sweep-v2.2`, `corne-cherry` and
`buspirate5-rev10`. By the nets that both routers complete, the medians of the four
pairs of runs: `tinytapeout-demo` +34 % vias and +18 % track, `glasgow-revD1` +6 % and
+5 %, `buspirate5-rev10` +13 % and +4 %, `glasgow-revC3` 11 % fewer vias and +5 % track,
`cynthion` 21 % fewer and level, `corne-cherry` 6 % less of both.

`tinytapeout-demo`'s runs end on the deadline and move from one measurement to the next:
0, 3, 0 and 0 open in the table's runs, 0, 7, 1 and 0 the second time, and 2, 0, 0 and 0
the third, beside 0.0.23 in the same five minutes, which had 2, 0, 0 and 0 with as many
vias (357 to 376 for 343 to 381).

With budgets of nodes, which do not move with the machine, one thread: the nine boards
that 40 million cut have 597 connections open over three orders for 591, eight of them
the same as with 0.0.23 to the order, and `corne-cherry` 36, 28 and 22 for 33, 21 and 26,
with 438 to 441 vias for 477 to 557. At 150 million, two orders: `corne-cherry` 8 and 0
for 0 and 2, with 430 and 456 vias for 503 and 504; `tinytapeout-demo` 0 and 0 with 320
and 329 vias, `glasgow-revC3` 6 and 6, `cynthion` 8 and 8, as 0.0.23. A via to the other
side was the cheap way back to a pour, and the dearer one costs `corne-cherry` a
connection where the nodes run out.

### Tried and not kept

- *The negotiation at cadlab's pitch on four layers* (`tinytapeout-demo`, above).
- *The pass for fewer vias given the time to end at the first pitch, the nets with
  planes first* (above): the board's vias the same.
- *Twenty and thirty steps for the via of a net with a plane* (above).

### What this does not show

More than four runs, but for `tinytapeout-demo`; other boards than cadlab's twelve; the
routers side by side on the generated boards. Not begun: the pass for fewer vias made by
the negotiation's own search on the routes as the passes and the later runs leave them,
which needs the claims of routes that are off its grid; cadlab's gridless pass, which
lays a track along the hull of what it passes (the nets with as many vias in both
routers are 5 % longer here on the boards at twice the pitch, and level at the first).

## PCB routing where cadlab's router was still ahead

What was changed after [the release below](#pcb-routing-faster-and-out-of-a-deeper-array),
0.0.22, with the aim of being level with cadlab's own router or ahead of it on every
measure on every board of its corpus, by taking from that router what it still did
better ([decisions](architecture.md#decisions)). That aim is not met: this says what
moved and what did not. Measured on 2026-11-22 with 0.0.23 from crates.io through
cadlab's adapter and its `route_bench`, on a machine of 64 threads that other work
shared: with budgets of nodes where two versions are compared, which do not depend on
the load, and with a deadline of 60 s for the table beside cadlab's router.

### A negotiation that does not end, on finer grids

`nrfmicro`, two layers, 85 connections, negotiated at its pitch of 0.2 mm, by the points
still shared after each round, in the order of the nets given: 597, 376, 290, 248, 217,
125, 95, 89, 87, 69, 65 and then 47 to 83 for twenty rounds, until twelve rounds had
gained nothing. Begun again four times in other orders, as 0.0.22 did, it ended the same
way, and the passes after it left a connection of the USB receptacle open in seven
orders of twelve. On the finer grids as they were first tried, the price stopped at
thirty steps there too, the same order: 0.133 mm down to 18 shared and no further,
0.1 mm to 22, and 0.067 mm, 1609 shared after its first round, to none in 23 rounds; in
other orders it is the grid of 0.1 mm that ends, or that of 0.133 mm. Through cadlab's
adapter at 60 s, one thread, twelve orders at once:

| | connections open, by order | orders with any open | time |
|---|---|---|---|
| 0.0.22 | 1, 1, 0, 0, 0, 1, 2, 2, 0, 0, 2, 2 | 7 | 6 to 59 s |
| the finer grids, with the price stopped at thirty steps there too | 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 2, 0 | 2 | 4 to 53 s |
| 0.0.23: the price rising to a thousand on the finer grids | 0 in all twelve | 0 | 5 to 34 s |

At 40 million nodes, three orders: none open for 2. With a via dearer on a finer grid
by what its steps are shorter, in the rounds too: 5 open over twelve orders for 2, so
only the routing again for fewer vias has it dearer.

### A via to a plane at twice the pitch

At twice the pitch, on more than two layers, a via cost every net ten steps, twice the
track that it costs at the first pitch and in cadlab's router. By the nets that both
routers close, one run of each at 60 s, `glasgow-revC3`'s nine nets with zones: 543 mm
of track and 91 vias, where cadlab's router has 485 mm and 120; with five steps for a
net with a plane, 431 mm and 122. At 100 million nodes, three orders, ten steps for
every net and five for those with planes:

| board | open | vias (median) | track (median) |
|---|---|---|---|
| `glasgow-revC3` | 7, 12, 8 → 6, 7, 10 | 434 → 408 | 3944 → 3830 mm |
| `tinytapeout-demo` | 8, 1, 0 → 0, 2, 0 | 365 → 372 | 5341 → 5451 mm |
| `cynthion` | 8, 8, 8 → 8, 8, 8 | 421 → 433 | 3155 → 3141 mm |
| `buspirate5-rev10` | 0, 0, 0 → 0, 0, 0 | 317 → 328 | 3115 → 3084 mm |

### Budgets of nodes, 0.0.22 and 0.0.23

40 million nodes, three orders, the nine boards that a budget cuts: 591 connections
open for 593 (`nrfmicro` 0 for 2, `tinytapeout-demo` 25 for 33, `glasgow-revC3` 42 for
34, `glasgow-revD1` 411, `corne-cherry` 80, `cynthion` 24 and `sweep-v2.2` 9 either way,
`buspirate5-rev10` and `lumenpnp-mobo` none). 150 million, two orders: `glasgow-revC3`
6 and 6 for 8 and 6, `corne-cherry` 0 and 2 and `tinytapeout-demo` 0 and 0 either way,
with 321 and 329 vias for 325 and 322.

### Beside cadlab's router at 60 s

Four runs of each, one of each at a time side by side: cadlab's router with seeds 1 to 4
on three threads, `guide` in four orders with three threads for its negotiation; a load
of 3 to 5 until the last five minutes of the half hour, 16 to 19 then. Connections open
by run, and vias and track by the medians:

| board | cadlab's router: open | `guide` 0.0.23: open | vias, `guide` against cadlab's | track |
|---|---|---|---|---|
| `buspirate-flash-sop` | 0, 0, 0, 0 | 0, 0, 0, 0 | 6 against 4 | 443 against 461 mm |
| `buspirate-rs232` | 0, 0, 0, 0 | 0, 0, 0, 0 | 5 against 10 | 300 against 281 mm |
| `lumenpnp-ringlight` | 0, 0, 0, 0 | 0, 0, 0, 0 | 4 against 3 | 167 against 177 mm |
| `nrfmicro` | 0, 0, 0, 0 | 0, 0, 0, 0 | 41.5 against 47.5 | 441 against 450 mm |
| `sweep-v2.2` | 3, 3, 3, 3 | 3, 3, 3, 3 | 21 against 25.5 | 2135 against 2159 mm |
| `corne-cherry` | 3, 3, 2, 4 | 0, 2, 2, 2 | 504 against 459 | 7433 against 8103 mm |
| `tinytapeout-demo` | 19, 2, 5, 5 | 1, 0, 0, 0 | 361.5 against 272.5 | 5419 against 4913 mm |
| `buspirate5-rev10` | 4, 2, 4, 8 | 0, 0, 0, 0 | 329 against 302 | 3081 against 3129 mm |
| `lumenpnp-mobo` | 9, 12, 7, 15 | 0, 0, 0, 0 | 337.5 against 342 | 5533 against 5448 mm |
| `cynthion` | 28, 29, 30, 28 | 8, 8, 8, 8 | 428.5 against 478 | 3284 against 3184 mm |
| `glasgow-revC3` | 36, 32, 40, 36 | 9, 7, 9, 9 | 415 against 418.5 | 3928 against 3449 mm |
| `glasgow-revD1` | 113, 169, 149, 135 | 95, 90, 30, 38 | 1202.5 against 1037 | 9376 against 8825 mm |
| all twelve | 215, 252, 240, 234 | 116, 110, 52, 60 | | |

No DRC error other than unrouted in the 96 runs. What is open is level or better on
every board, `glasgow-revD1` now in every run. Still behind: the vias of
`tinytapeout-demo` (+33 %), `glasgow-revD1` (+16 %), `corne-cherry` (+10 %) and
`buspirate5-rev10` (+9 %); the track of `glasgow-revC3` (+14 %, for +19 % with 0.0.22),
`tinytapeout-demo` (+10 %), `buspirate-rs232` (+7 %) and `glasgow-revD1` (+6 %); the
time of `corne-cherry` (52 to 59 s against 19 to 33 s), whose negotiation is ended by
its share and not by itself. A board's totals count the nets that only `guide` closes
(`glasgow-revC3`: 19 nets, 946 mm and 105 vias in one run); by the nets with no zone
that both routers close, one run of each, `guide` had 7 % more track and 27 % more vias
on `tinytapeout-demo`, 5 % and 5 % on `glasgow-revD1`, 2 % more and 14 % fewer on
`glasgow-revC3`, 1 % and 6 % on `buspirate5-rev10`, and less of both on `cynthion`.
`tinytapeout-demo`'s negotiation has some hundred points shared from its fifteenth round
to the end of its share: what cadlab's router's rounds do there that these do not was
not found.

**A deadline on a loaded machine.** The same measurement half an hour earlier, at a
load of 31 to 34, had 89 to 160 connections open for `guide` over the twelve boards and
281 to 317 for cadlab's router, and `tinytapeout-demo` 399 vias for 361.5. Beside 0.0.22
in one quarter hour at a load of 6 to 24, four orders: `tinytapeout-demo` 342 vias and
5353 mm for 329.5 and 5279, `lumenpnp-mobo` 337.5 and 5533 for 342.5 and 5576, none open
either way.

### Tried and not kept

- *The price of a shared point rising at the first pitch too, once four rounds have
  gained nothing.* `nrfmicro` the same. `corne-cherry` at 150 million nodes, three
  orders: 535 vias and 7736 mm with the price rising to a thousand, 541 and 7881 to a
  hundred, for 503 and 7411, with 7 and 1 connections open for 7.
- *A via dearer in the rounds on a finer grid* (above).

### What this does not show

More than four runs, or other boards than cadlab's twelve; a quiet machine for the whole
of a measurement; the routers side by side on the generated boards. The function that
lays a track no wider than the pad it meets, `pcb::neck_at_pads`, changes no route and
is not in these numbers: cadlab calls it on the result of either router, and with it no
pad of the twelve boards is met by a track wider than itself (91 were on `corne-cherry`
and 58 on `tinytapeout-demo` with `guide`'s routes before).

## PCB routing faster, and out of a deeper array

What was changed after [the release above](#pcb-routing-from-routes-with-none-moved),
0.0.21, for the board that a minute does not finish: `glasgow-revD1`, six layers, 1948
connections, an array of 256 balls. Its first run was stopped at its share of the minute
with some 120 connections open, three minutes left 23 and five minutes 14, all at the
array ([decisions](architecture.md#decisions)). Measured on 2026-11-22 through cadlab's
adapter and its `route_bench`, on a machine of 64 threads that other work shared: with
budgets of nodes where two versions of the code are compared, which do not depend on
the load, and with a deadline, two runs side by side, where the load was low (10 to 16).

### Where the time went

0.0.21, `glasgow-revD1`, 60 s, the negotiation on three threads, timed phase by phase
and round by round, and sampled (61 and 77 stacks of the running threads):

| | from | to | what |
|---|---|---|---|
| escapes, planes, global | 0 | 2.5 s | |
| negotiation | 2.5 s | 22.5 s | rounds of 6.4, 4.0, 4.2 and 4.3 s for 7.2, 13.5, 14.2 and 14.6 million nodes, a fifth cut by the share; 3469 points shared, 251 nets in another's way |
| what is shared taken out, routed again | 22.5 s | 25.4 s | 133 nets |
| passes | 25.4 s | 40.7 s | 13 million nodes, 0.86 million a second |
| cleanup, report | 40.7 s | 42.2 s | 117 connections open |

- *The rounds were on one thread, mostly.* The nets routed side by side were those
  whose regions were apart: 236 batches for 561 nets in the first round, 201 for 363 in
  the second.
- *A fifth of the searches takes nine tenths of the nodes.* In the second round 340
  searches of 1521 expand more than 20 thousand nodes each, 23.0 of 25.9 million
  together, and 64 of them more than 100 thousand, 10.3 million. They are connections
  across the board: one of 283 steps and four vias costs 392 where its bound says 290,
  and a search looks at every place on six layers that the difference pays for. Their
  targets are small (57 searches with a box of targets over 16 cells), so the bound is
  not what is loose about them.
- *The first round asks the board about 83 658 blocks of cells*, 3.4 s of its threads'
  time, each step from a cell asked both ways.
- *A lock.* The nets that a place is only for, round every pad, were in one map behind
  a lock that every search took for each such place: 7 % of the samples.
- *After the negotiation*, a net with no way where nothing is claimed looks at every
  free place of the board to say so: 612 thousand nodes for one net, 2.9 s for 133.
- *Late rounds.* Given the nodes (600 million), the rounds from the ninth on cost 9 to
  12 million each for some ninety nets, 120 thousand a net: at a price of a hundred
  steps for a shared point, a search that has to share looks at the board first.
- *The array.* `escape::plan` planned nothing for it: its nets' own via fits between
  four balls, so no ball was one for the smaller via, and each net's search put its via
  where its way was shortest.

`tinytapeout-demo`, the same way: fifty rounds in 19.6 s for 74.5 million nodes, its vias
203 after the first and 438 after the last, 380 once each net was routed again for
fewer; then 4.5 s of passes and settling and 8.4 s of cleanup; a first run of 35.2 s.

### What

- **The nets of a round, eight at a time.** Routed at once against the claims as they
  are before any of the eight, on as many threads as there are; then, in their order,
  each gives up its claims and claims what it was routed as if that is in no claim that
  it was not in then, and is routed again at its turn if it is. Eight on one thread as
  on several, so the routes are the same on any number.
- **No lock in a search.** The claims and the prices are atomic counters, the nets that
  a place is only for are kept by block of cells and set once.
- **A step asked one way.** The step back from a cell of the same block is the same
  track.
- **Late in a negotiation at twice the pitch a search looks round its two ends first**:
  from a price of five steps on, the box of the two ends and sixteen cells or half its longer side round
  it, and the whole board only with no way there. And a search after the negotiation,
  where nothing claimed is entered, has 100 thousand nodes or eight times what its net
  took last.
- **Of two nodes that cost the same, the one nearer the targets first.**
- **The vias of the balls deep in an array, where their nets' own via fits**: in the
  middle of four balls, on the side away from the array's middle, the outermost ring
  first, for the nets with no zone, in an array of twelve such balls and more most of
  which have a place for their own via; the smaller via where a track of the net passes
  between two of those side by side and not between two of its own.

### Results

*The same routes on one thread and on three.* `tinytapeout-demo`, 150 million nodes: 0
connections open, 328 vias, 5532.2 mm, 130.7 million nodes spent, on both, and the same
again with the steps asked both ways.

*What three threads make of a round*, by the nodes of its searches: the longest search
of each eight, or a third of the eight's nodes if that is more, and the nets routed
again on top.

| | routed again at their turn | of the nodes | two threads | three | four | six |
|---|---|---|---|---|---|---|
| `glasgow-revD1`, 18 rounds | 5 to 7 % of the nets | 10 to 13 % | 1.5 | 1.8 | 1.9 | 1.9 |
| `tinytapeout-demo`, 70 rounds | 10 % | 12 % | 1.6 | 1.9 | 2.0 | 2.0 |

*With a deadline*, `glasgow-revD1` at 60 s, 0.0.21 and this side by side, three threads
each, the machine's load 10 to 16:

| | rounds in the share | their times | nodes a second in them | points shared at the end | nets taken out | open after the first run | nodes of the first run | open at the end |
|---|---|---|---|---|---|---|---|---|
| 0.0.21 | 4 and a part | 6.4, 4.0, 4.2, 4.3 s | 3.4 million | 3469 | 133 | 116 to 117 | 78.7 million | 109 to 110 |
| now | 7 | 5.4, 3.0, 2.6, 2.7, 2.9, 1.7, 1.3 s | 4.2 million | 1107 | 80 | 21 to 40 | 97.9 million | 19 to 39 |

Two runs of each; the first run ends at its share, 42.2 s, in all four.

*With a budget of nodes*, one thread:

| `glasgow-revD1`, 500 million nodes | open after the first run | at the end |
|---|---|---|
| eight at a time, no more | 25 | 24 |
| the vias of the array, for every net | 12 | 12 |
| those, and the search round its two ends late | 5 | 5 |
| the vias for the nets with no zone, and the search round its ends | 3 | (not run to its end) |

At 40 million nodes, three orders of the nets, the nine boards a budget cuts, 0.0.21
and this behind the same adapter (cadlab at its commit `921ac7a`, whose fill differs
from the one of the tables above): 593 connections open for 615. `nrfmicro` 2 for 0,
`sweep-v2.2` 9 for 9, `corne-cherry` 80 for 81, `tinytapeout-demo` 33 for 23,
`buspirate5-rev10` and `lumenpnp-mobo` none either way, `cynthion` 24 for 24,
`glasgow-revC3` 34 for 41, `glasgow-revD1` 411 for 437: that board has no negotiation at
this budget, and the vias of its array alone are worth those 26. `tinytapeout-demo`'s
negotiation is cut by this budget in its first rounds, and what the nets routed again
at their turn cost, a tenth of the nodes, is rounds that it does not have; on the fill
before, the same two had 22 for 30.

At 150 million nodes, two orders: `corne-cherry` 0 and 2 open for 1 and 0,
`tinytapeout-demo` 0 and 0 either way with 325 and 322 vias for 353 and 364,
`glasgow-revC3` 8 and 6 for 9 and 9, `cynthion` 8 and 8 and `sweep-v2.2` 3 and 3 either
way, `buspirate5-rev10` and `lumenpnp-mobo` none.

### Tried and not kept

- **A net routed ahead of its turn, kept if nothing that its searches looked at has
  changed.** The routes are those of one net at a time exactly, and the threads are
  busy; but a search of 100 thousand nodes has looked at a sixth of the board, and 40 %
  of the nodes routed ahead were routed again with six nets ahead in turns, 80 to 90 %
  with the threads never waiting.
- **More nets at once where one of them took long the last time**, up to 48, so that
  the others fill the time of the longest: 12 % of the nets routed again for 6 %, and by
  the nodes 1.9 times as fast on three threads for 1.8, 2.2 on four for 1.9.
- **The search round its two ends at the first pitch too**, on two layers:
  `corne-cherry` at 150 million nodes over three orders, 6, 9 and 2 connections open
  with it, 4, 4 and 0 without, 2, 1 and 1 with 48 cells and the whole box round the
  ends, and 1, 2 and 1 with 0.0.21. The price stops at thirty steps there, and the ways
  round that such a board needs are long.
- **The search round its two ends in every round.** With 24 cells and half the box, the
  first eight rounds are the same to the point; with 12 and a quarter they cost 15 to
  20 % less, for as many points shared. It is where a price makes a search look far that
  it gains.
- **A way out of the array from each via on another layer**, ring by ring on the next
  inner layer, straight out along the vias' own row or along a line of balls beside
  it, kept as copper of its net if a route ends on it. The tracks are walls to every net
  that is not theirs: at 500 million nodes `glasgow-revD1` had 15 connections open for 5.
- **The vias of the array for the nets with zones too**: `cynthion` 9 connections open
  in each of three orders at 40 million nodes for 8, with 30 vias more, and
  `glasgow-revD1` 5 after its first run at 500 million for 3. **And in an array most of
  whose balls need the smaller via**, where the dog-bones were planned already:
  `glasgow-revC3` had 12 open after its first run at 150 million nodes for 7, and 8 at
  the end either way.
- **A dearer via late in the negotiation**, 20 and 30 steps for 10 once fewer points are
  shared than half the connections: `tinytapeout-demo` at 150 million nodes, two orders,
  326 and 352 vias, and 335 and 327, for 308 and 335, with 0 and 4 open, and 1 and 0,
  for 0 and 3. The wires routed late are few; the vias are made before.
- **Escapes only for the pads that no route of their net leaves, in a run that moves
  routes**, the board without the routes as before, so that a pad still shut in has its
  escape across another net's route and one that is out has none planned again.
  `cynthion`'s such runs route 13 nets again for 48, with no rip-up for 121, and end
  with the same board to the via (8 open, 422 vias) in 63 million nodes for 74; but
  `glasgow-revC3` over four orders at 150 million nodes has 8, 7, 7 and 6 open for 8, 6,
  7 and 4.

### As released

0.0.22 from crates.io behind cadlab's adapter (cadlab's D118), which also ends each
track where it is inside the pad of its net, so that the track is counted as cadlab's
router's is; beside that router on 2026-11-22: four runs of each at 60 s, three threads
each, one of each at a time, a load of 10 to 19.

- Open, the twelve boards: 63 to 168 for `guide` (median 90.5) against 234 to 294 (253);
  ahead in every run on `tinytapeout-demo` (none open in four orders, for 0, 6, 0 and 1
  with 0.0.21), `buspirate5-rev10`, `lumenpnp-mobo`, `cynthion` and `glasgow-revC3` (4 to
  10 against 32 to 40), level on four, ranges that overlap on `corne-cherry`,
  `glasgow-revD1` (39, 150, 77 and 65 against 144 to 184) and `nrfmicro` (1, 1, 0 and 0
  against 0, where 0.0.21 had none open), behind on none.
- `glasgow-revD1` with three minutes has 2 connections open and with five 2, for 23 and
  14. At a minute its first run still ends at its share, and the order with 150 had 40
  and 120 when run alone afterwards, with 98 and 62 million nodes in that run.
- Track by the medians within 2 % of cadlab's router's or under it on eight boards;
  `buspirate-rs232` +7 %, `tinytapeout-demo` +6 % (for +20 %), `glasgow-revD1` +5 %,
  `glasgow-revC3` +19 % (for +30 %). Vias: `tinytapeout-demo` 328.5 against 272.5, +21 %
  for +46 %; `glasgow-revD1` 1160.5 against 1004.5.
- Pads short of two spokes, by the host's new count: 36 to 51 over the four orders
  against 90 to 101.

### What this does not show

- A quiet machine for all of it: the times of the table with a deadline are from the
  quarter of an hour in which the load was 10 to 16, and at a load of 60 the same runs
  made four rounds in their share and left 140 to 195 open, 0.0.21 beside them 230.
- What the passes cost after the negotiation, 15 s of `glasgow-revD1`'s first run at
  0.86 million nodes a second on one thread: not touched.
- `tinytapeout-demo`'s vias: made in its first dozen rounds, and no price tried late
  takes them back.

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
| `lpddr4-testbed` | 57/57 | 344/344 | 0 | 233 → 220 (-) (248) | 647 → 670 (-) (956) | 90 (-) | 0 violations, 0 unconnected |
| `ddr5-testbed` | 66/66 | 255/255 | 0 | 190 → 170 (-) (180) | 843 → 876 (-) (1119) | 137 (-) | 0 violations, 0 unconnected |

The two DRAM boards, added for M6, are [below](#pcb-routing-of-dram-boards), and weren't
routed as tracks. *As tracks* is the router with `RouteOptions::planes` off, which routes zones' nets as any others
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
`RouteOptions::plane_cost`:

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
design, and with [`RouteOptions::pairs_first`](../src/pcb/mod.rs); it routes each board a second
time, apart and with no meanders, for the tables' *apart* columns, which `GUIDE_APART=0` leaves
out. Neither board has rules of its
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
| `lpddr4-testbed` | 57/57 | 344/344 | 0 | 233 → 220 (248) | 647 → 670 (956) | 90 | 0 violations, 0 unconnected |
| `ddr5-testbed` | 66/66 | 255/255 | 0 | 190 → 170 (180) | 843 → 876 (1119) | 137 | 0 violations, 0 unconnected |

Both are complete, with nothing wrong by KiCad's DRC, with our fills or KiCad's own, in 90 and
137 s, with fewer vias than by hand and less track, as the hand routing's buses are
meandered to their lengths and these have no lengths to meet. `ddr5-testbed` took 114 s before
pairs routed first could move, below: the larger budget of the last nets' searches costs it
time, and gives it 5 vias more and 32 mm less track.

### Constraints

With the rules above. *Coupled* is the share of a pair's length at its gap, as
[before](#pcb-differential-pairs-and-lengths); a skew in bold breaks its constraint.

| Board | Pair | guide: coupled | Length, mm | Skew | Vias | original: coupled | Length | Skew | Vias |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ddr5-testbed` | `DQS_P`, `DQS_N` | 53% | 23.68 | 0.0000 | 4 | 70% | 20.40 | 0.0000 | 4 |
| `ddr5-testbed` | `TDQS_P`, `TDQS_N` | 63% | 23.68 | 0.0000 | 2 | 15% | 20.40 | 0.0000 | 4 |
| `ddr5-testbed` | `CLK_P`, `CLK_N` | 58% | 21.13 | 0.0000 | 4 | 76% | 22.01 | 0.0000 | 4 |

| Board | Group | Nets | guide: lengths, mm | Skew | apart, no meanders: skew | original: lengths | Skew |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `lpddr4-testbed` | data | 22 | 11.10 | 0.000 | 4.563 | 10.23 to 10.24 | 0.010 |
| `lpddr4-testbed` | command | 10 | 15.23 | 0.000 | 4.425 | 14.13 to 14.17 | 0.044 |
| `ddr5-testbed` | data | 12 | 23.68 | 0.000 | 9.085 | 20.40, but `DQ2` 20.96 | **0.560** |
| `ddr5-testbed` | command | 17 | 21.13 | 0.000 | 8.596 | 22.01 to 22.26, but `CA12` 19.54, `CA13` 19.96 | **2.720** |

| Board | Nets | Connections | Pairs side by side | Constraints met | Meanders, mm | Seconds | KiCad |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `lpddr4-testbed` | 57/57 | 344/344 | - | 2 of 2 | 88.5 | 91 | 0 violations, 0 unconnected |
| `ddr5-testbed` | 66/66 | 255/255 | 3 of 3 | 2 of 2 | 162.8 | 204 | 0 violations, 0 unconnected |

**Both boards meet their constraints, and M6's exit is met.** `lpddr4-testbed` routes
completely, both its buses matched to within 50 nm. `ddr5-testbed` routes completely too, all
255 connections, with all three pairs side by side, `TDQS` for 63% of its length where the hand
routing has 15%, and both buses matched to within 50 nm, where the hand routing misses both by
0.56 and 2.7 mm. KiCad's DRC finds nothing wrong, and no connection missing. It takes 204 s,
measured with another routing run beside it on the machine.

Before this round, at commit `acfbc74`, three connections were left, `RESET_N`, `CA3` and a
ground finger, with `DQS` and `CLK` coupled for 73% and 64%. What completed it is pairs that
move: routed first, they stayed where they were, and `RESET_N`'s finger, beside `DQS`'s, was
walled in by `DQS_P`'s fan-out. Now a net left for the passes on the finer lattice may cross a
pair in rip-up like any route, and the pair is routed again around it, as below; `DQS` and `CLK`
give up some of their coupling to make the room, 53% and 58%.

### What it took

Before M6, both boards came out complete but for their ground, of 168 and 114 pads, which
failed whole, 9 and 7 connections short, in 100 and 120 s:

- **Budgets by connection.** A net's search budget, `RouteOptions::max_expansions`, is now for each
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
- **Pairs first** ([`RouteOptions::pairs_first`](../src/pcb/mod.rs)), off by default: routed last, no
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
- **Pairs that move in rip-up.** Pairs routed first were fixed copper. They are now routes of
  their nets, locked in the first pass, so that the rest is routed around them as before, and
  open in the passes on the finer lattice: a net's search in rip-up may cross a pair at the
  price of any route, and the pair is then taken out whole. Once the net is placed, the pair is
  routed again side by side, its own search in rip-up too, with every other pair and that net
  locked, and the nets its copper runs into are taken out to be routed again after it. A pair
  that finds no way goes back where it was, locked for good, and the net is routed again around
  it; a pair moved four times is locked too. On `ddr5-testbed`, `DQS` moved for the ground
  finger and for `RESET_N`, five times in all, each time finding a way; that left `CA3`.
- **More nodes for the last nets.** `CA3`'s search in rip-up ran out of its 2 million nodes on
  the finer lattice, where crossing routes and pairs at a price opens up far more of the board.
  There, such a search now has four times the budget, and `CA3` is routed. On the nine smaller
  boards and `lpddr4-testbed`, routed with their zones, nothing changes.

Neither board needed anything for blind, buried or micro vias, which neither allows or uses,
nor net topologies, as every signal of both is a point-to-point connection between the DRAM
and the edge; their BGAs needed no escape vias, as a 0.1 mm track passes between their balls.
Push-and-shove wasn't needed for completion without constraints: rip-up completes both.

What was tried for `ddr5-testbed`'s nets left with the pairs first, each measured on the board
with its constraints; the first two rows are kept, the rest dropped:

| Variation | Nets left | Pairs side by side | Constraints met |
| --- | --- | --- | --- |
| Pairs that move in rip-up, and four times the budget on the finer lattice | none | 3 of 3 | 2 of 2 |
| Pairs that move in rip-up alone | `CA3` | 3 of 3 | 2 of 2, of the nets routed |
| The same, with a third of the price for crossing a route on the finer lattice | `DQ5`, `CA6`, `CA7`, `CA9` | 3 of 3 | 2 of 2, of the nets routed |
| The same, with both | eight | 3 of 3 | 2 of 2, of the nets routed |
| Both kept, and nets of more than 16 pads routed a terminal at a time from the start, planes' too | `CA7`, `CA9` | 3 of 3 | 2 of 2, of the nets routed |
| A way out kept for every net's pins beside a pair's, not only those with zones | `CA7` | 1 of 3 | 1 of 2 |
| Before, pairs fixed | `RESET_N`, `CA3`, a ground finger | 3 of 3 | 2 of 2, of the nets routed |
| No way out kept for ground pins | `RESET_N`, `CA7`, `TEN`, a ground finger | 3 of 3 | 2 of 2, of the nets routed |
| A way out kept for pins of nets with zones within 2 mm of a pair's | `RESET_N`, `TEN` | 2 of 3 | 2 of 2 |
| The same, and nets of more than 16 pads routed a terminal at a time from the start, planes' too | `RESET_N`, `CA9`, `TEN` | 2 of 3 | 2 of 2, of the nets routed |
| A second run with the nets left routed first of all, before the pairs | `CA11`, `CA5`, `DQS_P` | 2 of 3 | 2 of 2, of the nets routed |
| The same, with four or six passes on the finer lattice | `CA11`, `CA5` | 2 of 3 | 1 of 2 |
| Pairs that a net left finds in its way moved, and routed again after it | three or four | 3 of 3 | 2 of 2, of the nets routed |

Of the variations from before pairs moved, those that leave two nets lose a pair, and the
second run doubles the time; moving pairs at the end of routing failed every time, as by then
no pair finds room again among routes that can't move. Moving them in rip-up differs in that:
the pair moves others in its turn. It costs time, 204 s against 150 before, as the passes on
the finer lattice route more and search further, and coupling, as above. Push-and-shove, which
M6 lists, and negotiating the last nets' conflicts at the detailed level weren't needed, and
weren't built.

### What this does not show

- Two boards of one DRAM each, point to point: no fly-by bus of several chips, no DIMM, and no
  FPGA or SoC end with its own BGA.
- The constraints are this test's, from the hand routing, not the designers' rules, which the
  boards don't have; impedance, and lengths inside the packages, aren't modelled.
- Results are chaotic, as for the hard boards above: every change above loses different nets,
  and one more change to the router may lose `ddr5-testbed` a net again.
- Times were measured with another routing run beside this one, on a machine shared with other
  work.

## PCB routing against Freerouting

This is the comparison that M5's exit asks for, and M6's: `pcb::route` and Freerouting 2.5.0
on the eleven [curated boards](../testdata/kicad/README.md), each given the same stripped board,
both results measured by the same code and checked by KiCad 10.0.3's own DRC.

Measured on 2026-11-10 with rustc 1.97.1, on an AMD Ryzen Threadripper PRO 9995WX shared with
other work: Freerouting's runs on commit `8b0bf72`, and guide's again on 2026-11-11 on commit
`07940f5`, after pairs routed first came to move in rip-up, which changes nothing here as no
board here routes its pairs first, and the last nets' searches got a larger budget, which
completes `ddr5-testbed` and gives `nk3-nrf-mini` 10 connections more. Freerouting is 2.5.0's jar on
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
| `GB-LIVE32` | guide | 79/79 | 197/197 | 0 | 0 | 142 | 2705 | 40.9 |
| | Freerouting | 79/79 | 197/197 | 4 track width | 0 | 168 | 2849 | 56.0; 485 on one thread |
| | tuned | 78/79 | 196/197 | 0 | 1 | 164 | 2859 | 32.2 |
| `nk3-nrf-mini` | guide | 29/30 | 82/83 | 0 | 1 | 58 | 297 | 50.1 |
| | Freerouting | 12/30 | 42/83 | 28 track width | 41 | 6 | 120 | 50.5 |
| | tuned | 11/30 | 36/83 | 3: 2 clearance, 1 mask bridge | 47 | 7 | 104 | 43.8 |
| `lpddr4-testbed` | guide | 57/57 | 344/344 | 0 | 0 | 127 | 1076 | 101 |
| | Freerouting | 50/57 | 337/344 | 75 track width | 7 | 120 | 1323 | 1020 |
| | tuned | 47/57 | 334/344 | 0 | 10 | 115 | 1235 | 550 |
| `ddr5-testbed` | guide | 66/66 | 255/255 | 0 | 0 | 145 | 1162 | 236 |
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

At commit `07940f5`, a net's search in rip-up on the finer lattice has four times the budget,
as [above](#pcb-routing-of-dram-boards). That completes `ddr5-testbed`'s ground, and gives
`nk3-nrf-mini` 82 connections and 29 nets, in 50 s; `lpddr4-testbed` has 3 vias and 5 mm
less. The other eight boards are as before.

### What the results say

- **Completion.** guide completes ten of the eleven boards, Freerouting seven in its default
  run and five tuned: guide completes `flea` and both DRAM boards too, and `udb-s` and
  `GB-LIVE32` against the tuned run. On every board, guide makes as many connections as
  Freerouting's best run or more: on `nk3-nrf-mini` 82 against 42 and 36, all but `/LED_R`, on
  `lpddr4-testbed` all 344 against 337 and 334, and on `ddr5-testbed` all 255 against 240 and
  245, every net against Freerouting's 51 and 58.
- **KiCad's DRC.** guide's results have no violation on any board. Freerouting's default run has
  some on eight of the eleven, 222 in all, nearly all of rules the DSN doesn't carry: tracks
  narrower than the board's minimum, from its neckdown at pads, edge clearances, mask openings,
  and `isi-channel`'s class vias, whose drill is under the board's minimum hole. Tuned, it has 22
  on two boards.
- **Track length.** guide's is shorter than Freerouting's, default and tuned, on every board
  but `nk3-nrf-mini`, where it makes many more connections, and `udb-s` against the tuned run,
  which leaves three out: on the seven boards both complete by default, 4484 mm against 4783,
  6%, and on the DRAM boards, with more connections, 19% and 2% shorter than the default run.
- **Vias.** guide has fewer than Freerouting's default on three boards, `isi-channel`, `udb-s`
  and `GB-LIVE32`, as many on one, and **more on seven**: `TS4231-BOB` by one, `lisn` 10 to 6,
  `placebo` 9 to 4, `flea` 50 to 38, `nk3-nrf-mini` with its many more connections, and the
  DRAM boards, 127 and 145 against 120 and 121, whose supplies, which Freerouting leaves in
  part, take a via at every pad.
- **Time, one thread each.** guide is faster on every board but `nk3-nrf-mini`, where the two
  take 50 s: 2 to 100 times on the small ones, 1.4 times on `GB-LIVE32` against Freerouting on
  all threads and 12 times on one, and 10 and 2.7 times on the DRAM boards, where Freerouting
  takes 17 and 10 minutes. On `nk3-nrf-mini`, guide routes 40 more connections in the same
  time.
- **Our DRC** now finds solder mask bridges too, and the two of `isi-channel`'s tuned run are
  among the violations it counts, as many as KiCad's; on `flea` and `nk3-nrf-mini` its counts of
  Freerouting's violations differ from KiCad's by one, which this comparison doesn't look into.

So on completion with no violation, guide ties Freerouting on the four boards that one of
Freerouting's runs completes with none, `GB-BRK-LINK-A`, `TS4231-BOB`, `lisn` and `placebo`,
and beats it on the other seven, where every run of Freerouting either makes fewer connections
or breaks rules, the DRAM boards included. It is shorter on all but `nk3-nrf-mini`, and faster
on all but that one, where they take as long, but has more vias on seven.

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
- Congestion feedback is only tested for doing what it should on a tiny case here; what it does
  to routability is measured on FPGAs, in [congestion](#congestion).
- Free-space problems are squares of one size. Mixed sizes, which PCBs have, are only tested for
  legality, and rows as tall as the tallest unit waste room when sizes differ much.

## PCB placement of the curated boards

This is the PCB half of M8's exit: the eleven [curated boards](../testdata/kicad/README.md), each
with the placement of its movable footprints discarded, placed again by
`pcb::place::place_components`, written back by `kicad::replace_placement`, and routed by
`pcb::route` with its zones, as [the routing through zones](#routing-through-zones) does; against
the same router on the board as its designer placed it. Measured on 2026-11-11 with rustc 1.97.1,
on commit `56adcf4`, one thread, on the AMD Ryzen Threadripper PRO 9995WX, shared with other
work, with the default options.

### How

```text
GUIDE_PLACE=1 GUIDE_KICAD=1 cargo test --release --features kicad curated_boards_place -- --nocapture
```

[`placement_tests.rs`](../src/kicad/placement_tests.rs) reads each board, builds its design
with `kicad::placement_design`, which decides what stays fixed, and places the rest from
nothing: no position or rotation of a movable footprint is used, only its side, and parts may
change sides where theirs is full. It checks that no two outlines overlap, writes the board,
reads it back, and checks that every footprint and pad is where the placer put it. Then it
takes the routing and zones' fills out, as the routing test does, routes the board, and checks
it:

- **our DRC**, `drc::check`, must find no violation involving a routed track or via or a pad of
  a footprint that moved, other than those the board has before routing, such as a footprint's
  own pads under its own opening in the mask;
- **KiCad's DRC**, `kicad-cli pcb drc` 10.0.3 at error severity, must find no violation
  involving a routed item, a footprint that moved or one of its pads, that it doesn't also find
  on the board as placed by hand and routed alike, or that is between items of one footprint.
  Courtyards that overlap, `courtyards_overlap`, are errors on every board, silkscreen that
  overlaps on `placebo` and `udb-s`, and thermal reliefs short of spokes, `starved_thermal`, on
  six;
- **completion**, by the router's count of connections, and KiCad's of missing ones.

What stays fixed, by `kicad::fixed_because`: locked footprints, those with no pads, those
nobody assembles (`board_only`, or out of both the bill of materials and the position files:
test and touch pads, card edges), those that draw the board's edge, mounting holes, connectors,
fiducials, switches and batteries by reference or library, and footprints whose courtyard
reaches the board's edge. That leaves nothing to place on `GB-BRK-LINK-A` and `isi-channel`,
which are connectors and holes, and on `flea`, a keyboard, only its microcontroller module and
power switch, as its keys are switches.

### Results

| Board | Movable (to the other side) | Placing, s | Wirelength, mm: by hand → placed | Connections: by hand, placed | Vias | Track, mm | Routing, s | Our DRC | KiCad |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 0/6 (0) | 0.00 | 98 → 98 | 8/8, 8/8 | 0 → 0 | 93 → 93 | 0.1 → 0.1 | 0 | 0 violations, 0 unconnected |
| `TS4231-BOB` | 5/6 (3) | 0.01 | 35 → 61 | 13/13, 13/13 | 0 → 11 | 31 → 71 | 0.1 → 0.7 | 0 | 0 violations, 0 unconnected |
| `lisn` | 28/36 (0) | 0.03 | 125 → 157 | 60/60, 60/60 | 7 → 14 | 144 → 253 | 0.5 → 0.7 | 0 | 0 violations, 0 unconnected |
| `isi-channel` | 0/17 (0) | 0.00 | 481 → 481 | 29/29, 29/29 | 0 → 0 | 453 → 453 | 0.1 → 0.1 | 0 | 0 violations, 0 unconnected |
| `placebo` | 10/18 (0) | 0.37 | 265 → 252 | 50/50, 50/50 | 9 → 17 | 292 → 257 | 0.4 → 0.6 | 0 | 0 violations, 0 unconnected |
| `udb-s` | 6/11 (3) | 0.05 | 103 → 102 | 33/33, 33/33 | 11 → 13 | 149 → 162 | 1.4 → 1.7 | 0 | 0 violations, 0 unconnected |
| `flea` | 2/19 (0) | 0.13 | 1376 → 1307 | 79/79, 79/79 | 52 → 47 | 1607 → 1521 | 5.6 → 1.5 | 0 | 0 violations, 0 unconnected |
| `GB-LIVE32` | 44/61 (0) | 0.02 | 2010 → 1900 | 197/197, 197/197 | 165 → 243 | 2283 → 2155 | 27.4 → 77.1 | 0 | 0 violations, 0 unconnected |
| `nk3-nrf-mini` | 21/35 (14) | 0.12 | 153 → 157 | 82/83, 83/83 | 55 → 63 | 249 → 305 | 22.2 → 283.4 | 0 | 0 violations, 0 unconnected |
| `lpddr4-testbed` | 62/68 (0) | 0.04 | 517 → 523 | 344/344, 344/344 | 220 → 219 | 670 → 684 | 100.9 → 116.7 | 0 | 0 violations, 0 unconnected |
| `ddr5-testbed` | 59/65 (0) | 0.03 | 789 → 761 | 255/255, 255/255 | 170 → 177 | 876 → 932 | 152.2 → 150.6 | 0 | 0 violations, 0 unconnected |

Wirelength is the half-perimeter of the nets that count, those that no zone connects, of the
parts where they were and where they are. Connections, vias and track are the router's, after
cleanup, as in [the routing through zones](#routing-through-zones), whose numbers for the boards
as placed by hand these are, run again. KiCad's column is what it finds that counts, as above.

### What the results say

- **Every board routes to completion**, and KiCad finds nothing wrong with what moved or was
  routed on any of them, courtyards and silkscreen included, and no connection missing. That
  includes `nk3-nrf-mini`, which as placed by hand leaves `/LED_R` unrouted, its LED inside an
  opening drawn in the mask that also exposes another net: placement keeps parts out of such
  openings, so the LED goes elsewhere, and all 83 connections are made. This is the exit M8 asks
  for.
- **Wirelength** comes out below the hand placement's on five boards of the nine with anything
  to place, from 1% (`udb-s`) to 5% (`GB-LIVE32`), and above on four: 1% on `lpddr4-testbed`,
  3% on `nk3-nrf-mini`, 26% on `lisn` and 74% on `TS4231-BOB`.
- **Routing it** takes more vias on seven of those nine boards, most on `GB-LIVE32`, 243 against
  165, whose 44 parts are spread with room between them where the designer packed them, and
  more track on six, most on `lisn`, 253 mm against 144, whose zones the designer drew around
  where each part was: placement only knows their boxes. Track is shorter on `placebo`, `flea`
  and `GB-LIVE32`. The DRAM boards, where the BGA and the edge connector stay and only their
  passives move, come within 2% and 6% of the hand placement's track, with as many vias give or
  take seven.
- **Sides.** `TS4231-BOB`, `udb-s` and `nk3-nrf-mini` have parts moved to the back, as their
  fronts would be more than 60% full with spacing: on `TS4231-BOB`, a board of 12 by 13 mm, that
  costs 11 vias where the designer's single-sided placement needs none; on `nk3-nrf-mini`, nothing
  fits on the front without it.
- **Time.** Placing takes at most 0.4 s; `placebo` tries less spacing and other fills before one
  fits. Routing the placed boards takes longer on most, three times as long on `GB-LIVE32` and
  13 times on `nk3-nrf-mini`, where rip-up works harder; shorter on `flea` and `ddr5-testbed`.

### What it took

Each of these was found by a board failing, by our DRC or KiCad's, or routing short:

- **Zones are where the designer drew them.** `lisn` is mostly copper pours on `F.Cu`, a net
  each, drawn round where each part was. Leaving out the wirelength of zones' nets, as planes,
  scattered parts away from their pours: 59 of 60 connections, `GND` unrouted, and 481 mm of
  track. Drawing each pin of a zone's net into the box of its zones, unless they cover most of
  the board, gave 60 of 60.
- **Pads past the courtyard.** `udb-s`'s connector draws its courtyard round its body only, and
  parts went on its pads: shorts. The room a footprint takes is now its courtyard and its pads.
- **Clearances.** `lisn`'s net class keeps 0.4 mm, more than two courtyards' margins: outlines
  grow by at least half the largest clearance, and fixed parts and keepouts as much.
- **Thermal spokes.** Pads of zones' nets packed against the edge or each other got one spoke,
  which KiCad reports on six boards: parts keep twice the spacing from the edge, so that the zone
  fills between, and 0.25 mm a side from each other, where 0.15 mm wasn't enough.
- **Silkscreen.** `placebo` and `udb-s` make overlapping silkscreen an error: there, outlines
  include the footprint's silkscreen, the fixed parts' and the board's are kept out of, and a
  moved footprint's text that would overlap goes beside it, or is hidden where nothing fits: two
  on `placebo`, one on `udb-s`.
- **Fixed parts without courtyards**, such as `placebo`'s cutout and `flea`'s keys, are kept
  out of by their pads, not their whole silkscreen, which on `placebo` left no room.
- **The back.** `nk3-nrf-mini` has no room on its front for its parts with spacing, nor without
  it for the legalizer, which needs some slack in a region cut up by its fixed parts: moving the
  smallest passives to the back, and trying fills a tenth either side of 60% before less
  spacing, fits it, with no spacing left. Its footprints are written flipped as KiCad flips
  them, which a test checks against the reader, pad by pad.
- **Openings in the mask** drawn on the board: on `nk3-nrf-mini`, a resistor went into the
  one that already bridges `/LED_R`. Kept out of, the board went from 70 of 83 connections to
  83.

### What this does not show

- Small boards: at most 62 parts to place. Global placement is analytic, which scales, but the
  passes and the rasterized free space have not been tried on a large board.
- The hand placements are better boards: shorter or as short, with fewer vias, on most of
  these. Placement knows nothing of signal integrity, assembly or mechanics beyond what stays
  fixed, and decoupling capacitors are found by their nets' names and tied to power pins in
  order, not to the pins that need them most.
- Sides are chosen by how full a side is, not by what is better electrically or for assembly,
  and parts turn by quarter turns only: free rotation is not done.
- Thermal grouping is a term the caller fills from what it knows of heat; no board here says.
- No other placer is compared: KiCad has none built in, and Freerouting doesn't place.

## Engineering changes

The roadmap's M9 asks for incremental re-routing after engineering changes, preserving existing
work, and for small netlist changes to cause small layout changes. These measurements take a
problem routed or placed, change it a little, and handle the change twice: from scratch, and
incrementally, from what was there. They compare what each changed, against the routes or the
placement before the change, and what each cost.

The measures, in `guide::eco` and `guide::place`:

- **Routes unchanged** (`RouteChange::unchanged_fraction`): of the nets routed both before and
  after, the fraction whose route has the same nodes.
- **Nodes changed** (`RouteChange::nodes_changed_fraction`): the `(net, node)` pairs added and
  removed over those nets, as a fraction of those before. On VPR's graphs and grids, nodes are
  the resources, so this is resources changed.
- **Items moved** (`PlacementChange::moved_fraction`): of the movable items placed both before
  and after, the fraction whose origin or rotation changed, and their **mean displacement**
  (`PlacementChange::mean_displacement`), the Manhattan distance moved, over all of them.

### How

Routing incrementally is `Engine::seed_routes` with the routes before, and with
`eco::stable_options`, unless a table says otherwise; placing incrementally is
`place::place_incremental` with the placement before as priors and its default options. From
scratch is the engine with its default options, and `place_analytic`.

The changes:

- **Nets**: of every hundred nets, 1 or 5 change, a third of them added, a third removed, and a
  third given one sink more. The routes before are of the problem before the change.
- **Cells blocked**: 10 or 50 cells of a grid that routes go through, other than terminals, are
  blocked, which cuts every route through them.
- **Blocks moved**, on VTR's circuits: 1% or 5% of the logic blocks, of those in no carry chain,
  swap places in pairs, each with one at most 4 tiles away, so that the terminals of their nets
  move. The routes before are of the design as placed.
- **Items added**, **blocks added**: 1% or 5% of the movable items, of those in no macro, are
  taken out with their pins by `place::restrict`, the rest is placed, and the whole problem is
  then placed incrementally from that, and from scratch.
- **Nets added**, for placement: the same with 1% or 5% of the nets.

The generated problems are three of each kind, at seeds 1 to 3:

```text
cargo test --release --lib engine::seed_tests::measure -- --ignored --nocapture
cargo test --release --lib place::eco_tests::measure -- --ignored --nocapture
```

The FPGA problems are VTR's circuits as VPR 9 packs and places them with seed 1, on the graph
at 1.3 times the channel width VPR needs on its placement, as in [FPGA placement against
VPR](#fpga-placement-against-vpr): three of the MCNC circuits and seven of the Verilog
benchmarks, each with changes at seeds 1 to 3. Routing is timing-driven with
`RrGraph::engine_options`, as for VTR's circuits elsewhere here, and incremental routing adds
`eco::stable_options` to those. Placement there is by wirelength alone, `place_analytic` against
`place_incremental`, as `place::restrict` can't take blocks out of `fpga::place_design`'s
problem, and each placement is then routed from scratch, for its wire and critical path:

```text
KEEP_GRAPHS=1 scripts/vtr/place.sh DIR WORK [CIRCUIT...]   # for the graphs
scripts/vtr/eco.sh WORK OUT [CIRCUIT...]                   # examples/vtr_eco.rs on each
python3 scripts/vtr/eco_tables.py OUT                      # the tables below
```

Times are on one thread of a shared machine of 192 threads, with other work running.

### Routing generated problems

Means over the three instances, with cost and expansions as ratios to routing from scratch.
The fabrics are the generated FPGA-like fabrics of the engine's tests and `benches/search.rs`,
and the grids have nets of two to four terminals within 16 cells of each other.

| Problem | Change | Way | Legal | Routes unchanged | Nodes changed | Cost vs scratch | Expansions vs scratch | Time (s) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Fabric 48x48, 1 track, 700 nets | 1% of nets | From scratch | 3 of 3 | 76.9% | 0.250 | 1.0000 | 1.000 | 0.1035 |
| Fabric 48x48, 1 track, 700 nets | 1% of nets | Seeds, defaults | 3 of 3 | 98.1% | 0.018 | 1.0003 | 0.023 | 0.0054 |
| Fabric 48x48, 1 track, 700 nets | 1% of nets | Seeds, incremental reroute | 3 of 3 | 98.8% | 0.012 | 1.0003 | 0.015 | 0.0044 |
| Fabric 48x48, 1 track, 700 nets | 1% of nets | Seeds and history | 3 of 3 | 98.1% | 0.019 | 1.0005 | 0.023 | 0.0054 |
| Fabric 48x48, 1 track, 700 nets | 1% of nets | `stable_options` | 3 of 3 | 99.9% | 0.001 | 1.0009 | 0.003 | 0.0029 |
| Fabric 48x48, 1 track, 700 nets | 1% of nets | Seeds, first present 10 | 3 of 3 | 99.9% | 0.001 | 1.0009 | 0.003 | 0.0028 |
| Fabric 64x64, 2 tracks, 1200 nets | 1% of nets | From scratch | 3 of 3 | 97.0% | 0.033 | 1.0000 | 1.000 | 0.4429 |
| Fabric 64x64, 2 tracks, 1200 nets | 1% of nets | Seeds, defaults | 3 of 3 | 99.7% | 0.001 | 0.9999 | 0.002 | 0.0079 |
| Fabric 64x64, 2 tracks, 1200 nets | 1% of nets | Seeds, incremental reroute | 3 of 3 | 99.7% | 0.001 | 0.9999 | 0.002 | 0.0075 |
| Fabric 64x64, 2 tracks, 1200 nets | 1% of nets | Seeds and history | 3 of 3 | 99.7% | 0.001 | 0.9999 | 0.002 | 0.0076 |
| Fabric 64x64, 2 tracks, 1200 nets | 1% of nets | `stable_options` | 3 of 3 | 99.8% | 0.001 | 0.9999 | 0.002 | 0.0074 |
| Fabric 64x64, 2 tracks, 1200 nets | 1% of nets | Seeds, first present 10 | 3 of 3 | 99.8% | 0.001 | 0.9999 | 0.002 | 0.0076 |
| Grid 256x256x2, 800 nets | 1% of nets | From scratch | 3 of 3 | 86.7% | 0.125 | 1.0000 | 1.000 | 0.1159 |
| Grid 256x256x2, 800 nets | 1% of nets | Seeds, defaults | 3 of 3 | 90.6% | 0.097 | 0.9994 | 0.068 | 0.0149 |
| Grid 256x256x2, 800 nets | 1% of nets | Seeds, incremental reroute | 3 of 3 | 94.3% | 0.045 | 0.9997 | 0.034 | 0.0112 |
| Grid 256x256x2, 800 nets | 1% of nets | Seeds and history | 3 of 3 | 91.3% | 0.090 | 1.0004 | 0.092 | 0.0169 |
| Grid 256x256x2, 800 nets | 1% of nets | `stable_options` | 3 of 3 | 98.7% | 0.008 | 1.0002 | 0.009 | 0.0081 |
| Grid 256x256x2, 800 nets | 1% of nets | Seeds, first present 10 | 3 of 3 | 97.0% | 0.026 | 1.0002 | 0.022 | 0.0094 |
| Fabric 48x48, 1 track, 700 nets | 5% of nets | From scratch | 3 of 3 | 63.3% | 0.404 | 1.0000 | 1.000 | 0.1138 |
| Fabric 48x48, 1 track, 700 nets | 5% of nets | Seeds, defaults | 3 of 3 | 93.4% | 0.060 | 1.0005 | 0.065 | 0.0106 |
| Fabric 48x48, 1 track, 700 nets | 5% of nets | Seeds, incremental reroute | 3 of 3 | 94.0% | 0.047 | 1.0011 | 0.050 | 0.0091 |
| Fabric 48x48, 1 track, 700 nets | 5% of nets | Seeds and history | 3 of 3 | 93.2% | 0.060 | 1.0018 | 0.077 | 0.0128 |
| Fabric 48x48, 1 track, 700 nets | 5% of nets | `stable_options` | 3 of 3 | 99.0% | 0.005 | 1.0044 | 0.012 | 0.0045 |
| Fabric 48x48, 1 track, 700 nets | 5% of nets | Seeds, first present 10 | 3 of 3 | 99.0% | 0.005 | 1.0044 | 0.012 | 0.0045 |
| Fabric 64x64, 2 tracks, 1200 nets | 5% of nets | From scratch | 3 of 3 | 90.1% | 0.105 | 1.0000 | 1.000 | 0.4497 |
| Fabric 64x64, 2 tracks, 1200 nets | 5% of nets | Seeds, defaults | 3 of 3 | 98.3% | 0.011 | 1.0002 | 0.028 | 0.0186 |
| Fabric 64x64, 2 tracks, 1200 nets | 5% of nets | Seeds, incremental reroute | 3 of 3 | 98.4% | 0.010 | 1.0001 | 0.026 | 0.0176 |
| Fabric 64x64, 2 tracks, 1200 nets | 5% of nets | Seeds and history | 3 of 3 | 98.3% | 0.011 | 1.0002 | 0.028 | 0.0179 |
| Fabric 64x64, 2 tracks, 1200 nets | 5% of nets | `stable_options` | 3 of 3 | 99.1% | 0.004 | 1.0005 | 0.012 | 0.0116 |
| Fabric 64x64, 2 tracks, 1200 nets | 5% of nets | Seeds, first present 10 | 3 of 3 | 99.1% | 0.004 | 1.0005 | 0.012 | 0.0117 |
| Grid 256x256x2, 800 nets | 5% of nets | From scratch | 3 of 3 | 59.6% | 0.403 | 1.0000 | 1.000 | 0.1136 |
| Grid 256x256x2, 800 nets | 5% of nets | Seeds, defaults | 3 of 3 | 73.0% | 0.281 | 0.9978 | 0.202 | 0.0296 |
| Grid 256x256x2, 800 nets | 5% of nets | Seeds, incremental reroute | 3 of 3 | 81.5% | 0.167 | 0.9991 | 0.113 | 0.0205 |
| Grid 256x256x2, 800 nets | 5% of nets | Seeds and history | 3 of 3 | 72.6% | 0.273 | 1.0013 | 0.289 | 0.0382 |
| Grid 256x256x2, 800 nets | 5% of nets | `stable_options` | 3 of 3 | 93.9% | 0.048 | 1.0008 | 0.043 | 0.0118 |
| Grid 256x256x2, 800 nets | 5% of nets | Seeds, first present 10 | 3 of 3 | 89.1% | 0.107 | 0.9999 | 0.077 | 0.0152 |
| Grid 256x256x2, 800 nets | 10 cells blocked | From scratch | 3 of 3 | 94.6% | 0.044 | 1.0000 | 1.000 | 0.1156 |
| Grid 256x256x2, 800 nets | 10 cells blocked | Seeds, defaults | 3 of 3 | 87.0% | 0.133 | 0.9992 | 0.080 | 0.0164 |
| Grid 256x256x2, 800 nets | 10 cells blocked | Seeds, incremental reroute | 3 of 3 | 92.6% | 0.064 | 0.9994 | 0.037 | 0.0119 |
| Grid 256x256x2, 800 nets | 10 cells blocked | Seeds and history | 3 of 3 | 91.4% | 0.080 | 1.0002 | 0.085 | 0.0165 |
| Grid 256x256x2, 800 nets | 10 cells blocked | `stable_options` | 3 of 3 | 98.8% | 0.010 | 1.0000 | 0.001 | 0.0068 |
| Grid 256x256x2, 800 nets | 10 cells blocked | Seeds, first present 10 | 3 of 3 | 98.8% | 0.010 | 1.0000 | 0.001 | 0.0068 |
| Grid 256x256x2, 800 nets | 50 cells blocked | From scratch | 3 of 3 | 77.2% | 0.184 | 1.0000 | 1.000 | 0.1190 |
| Grid 256x256x2, 800 nets | 50 cells blocked | Seeds, defaults | 3 of 3 | 62.5% | 0.376 | 0.9978 | 0.254 | 0.0385 |
| Grid 256x256x2, 800 nets | 50 cells blocked | Seeds, incremental reroute | 3 of 3 | 75.6% | 0.195 | 0.9981 | 0.125 | 0.0237 |
| Grid 256x256x2, 800 nets | 50 cells blocked | Seeds and history | 3 of 3 | 67.0% | 0.306 | 1.0007 | 0.324 | 0.0439 |
| Grid 256x256x2, 800 nets | 50 cells blocked | `stable_options` | 3 of 3 | 93.7% | 0.045 | 1.0001 | 0.012 | 0.0085 |
| Grid 256x256x2, 800 nets | 50 cells blocked | Seeds, first present 10 | 3 of 3 | 93.4% | 0.049 | 1.0001 | 0.014 | 0.0086 |

- **Routing from scratch is not stable**, even though it is deterministic: the order of the nets
  and the negotiation spread a small change over the whole routing. It kept 60% to 97% of the
  routes, and the more congested the problem, the fewer.
- **`stable_options` kept 94% to 99.9%**, with nodes changed down from 0.03–0.40 to
  0.001–0.05 of those before, for a cost within half a percent of routing from scratch, in 0.1%
  to 4.3% of its expansions and 2% to 10% of its time.
- **Seeds with the default options are not enough** on the grids: a net that changed routes at a
  present factor of 0.5 in the first iteration, takes what unchanged nets hold, and sets off a
  negotiation, so that with 50 cells blocked they kept fewer routes than routing from scratch
  did, 62.5% against 77%. A first present factor of 10 makes changed nets go around unchanged
  ones where they can, and incremental rerouting keeps what of a moved net is not in a conflict;
  together they do better than either alone. A first present factor of 3 kept fewer routes, and
  1000 took up to 2.5 times the expansions of routing from scratch, as prices that high make a
  search's lower bound weak.
- **History** carried over from the run before did not help, and cost up to 43% more
  expansions than seeds alone, so `stable_options` leaves it out.

### Placing generated problems

Means over the three instances. Wirelength is against placing from scratch, and the method's
number is the displacement factor, `IncrementalOptions::displacement_permille`, of which 1000 is
the default. **From scratch, another seed** places the changed problem from scratch with another
seed, which shows how much a placement moves for nothing, and how far apart two placements from
scratch are in wirelength.

| Problem | Change | Method | Wirelength vs scratch | Items moved | Mean displacement | Max | Time (s) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Peko 32x32 | 1% items added | From scratch | 1.000 | 99.3% | 18.25 | 61 | 0.365 |
| Peko 32x32 | 1% items added | From scratch, another seed | 1.050 | 99.7% | 24.27 | 60 | 0.367 |
| Peko 32x32 | 1% items added | Incremental, 0 | 1.023 | 76.6% | 1.75 | 29 | 0.122 |
| Peko 32x32 | 1% items added | Incremental, 300 | 1.018 | 57.5% | 0.93 | 33 | 0.120 |
| Peko 32x32 | 1% items added | Incremental, 1000 | 1.038 | 9.3% | 0.15 | 9 | 0.090 |
| Peko 32x32 | 5% items added | From scratch | 1.000 | 99.7% | 23.74 | 62 | 0.358 |
| Peko 32x32 | 5% items added | From scratch, another seed | 1.050 | 99.9% | 20.89 | 59 | 0.366 |
| Peko 32x32 | 5% items added | Incremental, 0 | 1.071 | 81.3% | 1.99 | 35 | 0.120 |
| Peko 32x32 | 5% items added | Incremental, 300 | 1.083 | 64.9% | 1.13 | 31 | 0.117 |
| Peko 32x32 | 5% items added | Incremental, 1000 | 1.148 | 25.0% | 0.38 | 18 | 0.095 |
| Peko 32x32 | 1% nets added | From scratch | 1.000 | 98.9% | 7.64 | 40 | 0.356 |
| Peko 32x32 | 1% nets added | From scratch, another seed | 1.050 | 99.8% | 19.62 | 58 | 0.362 |
| Peko 32x32 | 1% nets added | Incremental, 0 | 1.042 | 54.1% | 1.22 | 38 | 0.121 |
| Peko 32x32 | 1% nets added | Incremental, 300 | 1.052 | 0.0% | 0.00 | 0 | 0.118 |
| Peko 32x32 | 1% nets added | Incremental, 1000 | 1.052 | 0.0% | 0.00 | 0 | 0.089 |
| Peko 32x32 | 5% nets added | From scratch | 1.000 | 99.8% | 17.38 | 51 | 0.355 |
| Peko 32x32 | 5% nets added | From scratch, another seed | 1.050 | 99.9% | 22.35 | 56 | 0.361 |
| Peko 32x32 | 5% nets added | Incremental, 0 | 1.053 | 80.5% | 2.08 | 43 | 0.123 |
| Peko 32x32 | 5% nets added | Incremental, 300 | 1.081 | 39.5% | 0.74 | 33 | 0.116 |
| Peko 32x32 | 5% nets added | Incremental, 1000 | 1.129 | 0.0% | 0.00 | 0 | 0.092 |
| Peko 32x32, typed, pads | 1% items added | From scratch | 1.000 | 65.7% | 2.27 | 29 | 0.374 |
| Peko 32x32, typed, pads | 1% items added | From scratch, another seed | 1.028 | 69.0% | 2.51 | 33 | 0.379 |
| Peko 32x32, typed, pads | 1% items added | Incremental, 0 | 1.010 | 51.8% | 1.30 | 14 | 0.127 |
| Peko 32x32, typed, pads | 1% items added | Incremental, 300 | 1.009 | 34.4% | 0.64 | 11 | 0.127 |
| Peko 32x32, typed, pads | 1% items added | Incremental, 1000 | 1.018 | 9.3% | 0.18 | 17 | 0.108 |
| Peko 32x32, typed, pads | 5% items added | From scratch | 1.000 | 67.5% | 2.36 | 52 | 0.372 |
| Peko 32x32, typed, pads | 5% items added | From scratch, another seed | 1.028 | 71.4% | 2.54 | 52 | 0.372 |
| Peko 32x32, typed, pads | 5% items added | Incremental, 0 | 1.008 | 52.6% | 1.33 | 52 | 0.126 |
| Peko 32x32, typed, pads | 5% items added | Incremental, 300 | 0.998 | 40.3% | 0.84 | 52 | 0.123 |
| Peko 32x32, typed, pads | 5% items added | Incremental, 1000 | 1.048 | 16.8% | 0.34 | 21 | 0.106 |
| Peko 32x32, typed, pads | 1% nets added | From scratch | 1.000 | 62.7% | 1.79 | 36 | 0.372 |
| Peko 32x32, typed, pads | 1% nets added | From scratch, another seed | 1.028 | 66.1% | 2.02 | 34 | 0.370 |
| Peko 32x32, typed, pads | 1% nets added | Incremental, 0 | 1.018 | 15.8% | 0.36 | 15 | 0.130 |
| Peko 32x32, typed, pads | 1% nets added | Incremental, 300 | 1.020 | 0.0% | 0.00 | 0 | 0.127 |
| Peko 32x32, typed, pads | 1% nets added | Incremental, 1000 | 1.020 | 0.0% | 0.00 | 0 | 0.108 |
| Peko 32x32, typed, pads | 5% nets added | From scratch | 1.000 | 65.2% | 2.05 | 36 | 0.374 |
| Peko 32x32, typed, pads | 5% nets added | From scratch, another seed | 1.028 | 69.1% | 2.30 | 36 | 0.373 |
| Peko 32x32, typed, pads | 5% nets added | Incremental, 0 | 1.016 | 52.6% | 1.34 | 36 | 0.130 |
| Peko 32x32, typed, pads | 5% nets added | Incremental, 300 | 1.039 | 11.7% | 0.25 | 29 | 0.128 |
| Peko 32x32, typed, pads | 5% nets added | Incremental, 1000 | 1.055 | 0.0% | 0.00 | 0 | 0.108 |
| Free grid 16x16, room | 1% items added | From scratch | 1.000 | 100.0% | 116.68 | 190 | 0.125 |
| Free grid 16x16, room | 1% items added | From scratch, another seed | 0.919 | 99.7% | 131.38 | 340 | 0.129 |
| Free grid 16x16, room | 1% items added | Incremental, 0 | 0.944 | 33.6% | 2.84 | 25 | 0.011 |
| Free grid 16x16, room | 1% items added | Incremental, 300 | 0.975 | 8.4% | 0.51 | 20 | 0.011 |
| Free grid 16x16, room | 1% items added | Incremental, 1000 | 0.983 | 0.4% | 0.04 | 20 | 0.012 |
| Free grid 16x16, room | 5% items added | From scratch | 1.000 | 100.0% | 147.72 | 355 | 0.127 |
| Free grid 16x16, room | 5% items added | From scratch, another seed | 0.919 | 100.0% | 131.75 | 335 | 0.133 |
| Free grid 16x16, room | 5% items added | Incremental, 0 | 1.149 | 57.9% | 6.56 | 95 | 0.014 |
| Free grid 16x16, room | 5% items added | Incremental, 300 | 1.196 | 18.4% | 1.50 | 45 | 0.013 |
| Free grid 16x16, room | 5% items added | Incremental, 1000 | 1.215 | 3.0% | 0.24 | 20 | 0.013 |
| Free grid 16x16, room | 1% nets added | From scratch | 1.000 | 99.9% | 90.10 | 185 | 0.128 |
| Free grid 16x16, room | 1% nets added | From scratch, another seed | 0.919 | 99.2% | 117.81 | 340 | 0.133 |
| Free grid 16x16, room | 1% nets added | Incremental, 0 | 0.958 | 21.5% | 1.69 | 25 | 0.012 |
| Free grid 16x16, room | 1% nets added | Incremental, 300 | 0.973 | 8.7% | 0.51 | 15 | 0.012 |
| Free grid 16x16, room | 1% nets added | Incremental, 1000 | 0.981 | 0.0% | 0.00 | 0 | 0.012 |
| Free grid 16x16, room | 5% nets added | From scratch | 1.000 | 100.0% | 125.09 | 190 | 0.128 |
| Free grid 16x16, room | 5% nets added | From scratch, another seed | 0.919 | 99.9% | 115.70 | 345 | 0.133 |
| Free grid 16x16, room | 5% nets added | Incremental, 0 | 0.996 | 63.9% | 6.15 | 150 | 0.013 |
| Free grid 16x16, room | 5% nets added | Incremental, 300 | 1.029 | 22.0% | 1.58 | 200 | 0.012 |
| Free grid 16x16, room | 5% nets added | Incremental, 1000 | 1.057 | 1.2% | 0.31 | 200 | 0.012 |

- **Placing from scratch moves nearly everything**: 63% to 100% of the items, even when the
  problem changed by one item in a hundred, and analytic placement from another seed is 3% to 5%
  longer on Peko problems and 8% shorter on the free grid. That is the noise against which the
  wirelength of the incremental placements is to be read.
- **At the default factor, incremental placement moved 0% to 25% of the items**, a mean of at
  most 0.4 units, for wirelength from 2% below to 15% above that from scratch, but for the weak
  case below, in a quarter of the time on Peko problems and a tenth in free space. Where only
  nets were added, it moved nothing or nearly: no single move gains as much as it costs, and the
  price is that of the placement before, which never saw those nets.
- **The factor trades one for the other.** At 0, items move freely in the cold, short-range
  annealing, and 16% to 81% of them moved, by 0.4 to 7 units on average, for wirelength within
  1% to 7% of that from scratch on Peko problems. 300 is in between.
- **Free space with 5% of the items added is the weak case**: 15% to 22% above placing from
  scratch, whatever the factor. The items added go to the free spot nearest to the median of
  what they connect to, which in a packed placement can be far from it, and annealing over short
  ranges does not bring them back.

### VTR's circuits

Unchanged and nodes changed are means over the three seeds, and wire, critical path, expansions
and time are geometric means over them of incremental against from scratch. Legal counts the
seeds legal from scratch and incrementally. Means are over the circuits, geometric for ratios.

#### Nets changed

| Circuit | Changed | Unchanged: scratch | Unchanged: incremental | Nodes changed: scratch | Nodes changed: incremental | Wire | Critical path | Expansions | Time | Legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| tseng | 1% | 76.9% | 99.8% | 0.248 | 0.001 | 1.001 | 1.000 | 0.002 | 0.218 | 3/3 |
| clma | 1% | 33.5% | 99.2% | 0.662 | 0.002 | 1.001 | 1.000 | 0.003 | 0.146 | 3/3 |
| ex1010 | 1% | 20.5% | 98.4% | 0.931 | 0.004 | 0.997 | 1.000 | 0.003 | 0.078 | 3/3 |
| ch_intrinsics | 1% | 91.1% | 99.1% | 0.066 | 0.005 | 1.003 | 1.000 | 0.002 | 0.382 | 3/3 |
| diffeq1 | 1% | 70.8% | 99.2% | 0.257 | 0.005 | 1.013 | 1.013 | 0.004 | 0.131 | 3/3 |
| blob_merge | 1% | 42.7% | 99.0% | 0.653 | 0.003 | 1.001 | 1.000 | 0.005 | 0.220 | 3/3 |
| or1200 | 1% | 50.2% | 97.8% | 0.490 | 0.010 | 1.004 | 1.004 | 0.010 | 0.255 | 3/3 |
| stereovision2 | 1% | 70.1% | 99.5% | 0.299 | 0.003 | 1.001 | 1.000 | 0.006 | 0.308 | 3/3 |
| bgm | 1% | 38.6% | 96.8% | 0.623 | 0.013 | 1.001 | 1.000 | 0.014 | 0.203 | 3/3 |
| LU8PEEng | 1% | 46.9% | 99.3% | 0.494 | 0.002 | 1.000 | 1.000 | 0.005 | 0.205 | 3/3 |
| **Mean** | 1% | 54.1% | 98.8% | 0.472 | 0.005 | 1.002 | 1.002 | 0.004 | 0.197 | |
| tseng | 5% | 60.4% | 97.7% | 0.454 | 0.016 | 1.009 | 1.000 | 0.022 | 0.285 | 3/3 |
| clma | 5% | 19.7% | 91.7% | 0.904 | 0.041 | 1.013 | 1.000 | 0.034 | 0.240 | 3/3 |
| ex1010 | 5% | 12.4% | 79.6% | 1.065 | 0.138 | 1.006 | 0.999 | 0.079 | 0.240 | 3/3 |
| ch_intrinsics | 5% | 82.6% | 98.7% | 0.132 | 0.007 | 1.001 | 1.000 | 0.024 | 0.437 | 3/3 |
| diffeq1 | 5% | 58.5% | 94.3% | 0.362 | 0.043 | 1.004 | 1.010 | 0.078 | 0.216 | 3/3 |
| blob_merge | 5% | 34.3% | 96.6% | 0.753 | 0.008 | 1.011 | 1.000 | 0.023 | 0.266 | 3/3 |
| or1200 | 5% | 40.1% | 93.3% | 0.606 | 0.030 | 1.001 | 1.000 | 0.045 | 0.310 | 3/3 |
| stereovision2 | 5% | 56.5% | 97.9% | 0.431 | 0.015 | 1.004 | 1.000 | 0.031 | 0.338 | 3/3 |
| bgm | 5% | 29.4% | 90.7% | 0.737 | 0.046 | 1.006 | 1.000 | 0.050 | 0.265 | 3/3 |
| LU8PEEng | 5% | 36.6% | 96.7% | 0.623 | 0.013 | 1.005 | 1.000 | 0.028 | 0.265 | 3/3 |
| **Mean** | 5% | 43.0% | 93.7% | 0.607 | 0.036 | 1.006 | 1.001 | 0.037 | 0.281 | |

#### Blocks moved

| Circuit | Changed | Unchanged: scratch | Unchanged: incremental | Nodes changed: scratch | Nodes changed: incremental | Wire | Critical path | Expansions | Time | Legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| tseng | 1% | 96.8% | 99.6% | 0.030 | 0.005 | 1.001 | 1.000 | 0.004 | 0.197 | 3/3 |
| clma | 1% | 17.6% | 89.9% | 0.977 | 0.060 | 0.999 | 0.999 | 0.027 | 0.221 | 3/3 |
| ex1010 | 1% | 12.6% | 79.8% | 1.080 | 0.128 | 1.001 | 1.001 | 0.066 | 0.208 | 3/3 |
| ch_intrinsics | 1% | 88.7% | 98.4% | 0.107 | 0.021 | 0.999 | 1.000 | 0.005 | 0.410 | 3/3 |
| diffeq1 | 1% | 65.1% | 96.9% | 0.311 | 0.029 | 1.000 | 0.996 | 0.013 | 0.160 | 3/3 |
| blob_merge | 1% | 27.7% | 91.0% | 0.858 | 0.047 | 1.008 | 1.005 | 0.026 | 0.225 | 3/3 |
| or1200 | 1% | 68.9% | 98.7% | 0.279 | 0.011 | 1.003 | 1.000 | 0.003 | 0.198 | 3/3 |
| stereovision2 | 1% | 71.7% | 98.9% | 0.300 | 0.016 | 1.001 | 1.000 | 0.011 | 0.232 | 3/3 |
| bgm | 1% | 32.1% | 91.7% | 0.722 | 0.055 | 1.005 | 0.999 | 0.040 | 0.224 | 3/3 |
| LU8PEEng | 1% | 39.9% | 95.5% | 0.599 | 0.029 | 1.002 | 1.000 | 0.017 | 0.243 | 3/3 |
| **Mean** | 1% | 52.1% | 94.0% | 0.526 | 0.040 | 1.002 | 1.000 | 0.014 | 0.225 | |
| tseng | 5% | 73.0% | 97.4% | 0.307 | 0.031 | 0.996 | 1.000 | 0.046 | 0.288 | 3/3 |
| clma | 5% | 12.2% | 74.2% | 1.098 | 0.188 | 1.010 | 1.000 | 0.092 | 0.294 | 3/3 |
| ex1010 | 5% | 7.6% | 59.2% | 1.196 | 0.372 | 1.009 | 1.001 | 0.188 | 0.336 | 3/3 |
| ch_intrinsics | 5% | 69.7% | 93.7% | 0.279 | 0.082 | 1.008 | 1.000 | 0.049 | 0.429 | 3/3 |
| diffeq1 | 5% | 56.8% | 94.0% | 0.392 | 0.062 | 1.004 | 0.994 | 0.062 | 0.207 | 3/3 |
| blob_merge | 5% | 20.6% | 74.4% | 0.974 | 0.213 | 1.029 | 1.005 | 0.121 | 0.412 | 3/3 |
| or1200 | 5% | 50.0% | 96.2% | 0.491 | 0.033 | 1.006 | 1.000 | 0.018 | 0.219 | 3/3 |
| stereovision2 | 5% | 57.9% | 95.5% | 0.455 | 0.069 | 1.006 | 1.000 | 0.051 | 0.321 | 3/3 |
| bgm | 5% | 20.9% | 73.1% | 0.900 | 0.227 | 1.014 | 1.000 | 0.148 | 0.348 | 3/3 |
| LU8PEEng | 5% | 25.5% | 83.0% | 0.821 | 0.133 | 1.014 | 1.000 | 0.067 | 0.282 | 3/3 |
| **Mean** | 5% | 39.4% | 84.1% | 0.691 | 0.141 | 1.009 | 1.000 | 0.069 | 0.306 | |

#### Blocks added

| Circuit | Added | Moved: scratch | Moved: incremental | Mean displacement: scratch | Mean displacement: incremental | BB estimate | Routed wire | Critical path | Place time | Legal |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| tseng | 2 | 92.6% | 0.1% | 6.71 | 0.002 | 0.991 | 0.982 | 0.992 | 0.333 | 3/3 |
| clma | 5 | 93.4% | 23.2% | 8.78 | 0.710 | 1.012 | 1.023 | 1.004 | 0.267 | 3/3 |
| ex1010 | 4 | 99.3% | 10.8% | 11.76 | 0.159 | 1.002 | 1.028 | 0.932 | 0.282 | 3/3 |
| ch_intrinsics | 3 | 92.1% | 37.1% | 7.46 | 2.969 | 1.001 | 1.002 | 0.980 | 0.332 | 3/3 |
| diffeq1 | 3 | 93.4% | 0.0% | 8.97 | 0.000 | 0.993 | 1.030 | 1.003 | 0.366 | 3/3 |
| blob_merge | 7 | 98.8% | 44.2% | 15.58 | 2.108 | 1.019 | 1.015 | 1.016 | 0.302 | 3/3 |
| or1200 | 8 | 97.2% | 0.0% | 16.19 | 0.000 | 0.992 | 0.997 | 1.020 | 0.348 | 3/3 |
| stereovision2 | 14 | 99.4% | 0.0% | 47.29 | 0.000 | 1.010 | 1.010 | 0.995 | 0.318 | 3/3 |
| bgm | 28 | 98.7% | 42.5% | 27.74 | 0.920 | 1.002 | 1.001 | 0.974 | 0.325 | 3/3 |
| LU8PEEng | 21 | 98.0% | 43.6% | 26.91 | 1.218 | 1.001 | 1.000 | 0.992 | 0.311 | 3/3 |
| **Mean** | 1% | 96.3% | 20.1% | 17.74 | 0.808 | 1.002 | 1.009 | 0.990 | 0.317 | |
| tseng | 9 | 95.8% | 0.0% | 8.80 | 0.000 | 0.998 | 0.984 | 0.987 | 0.338 | 3/3 |
| clma | 22 | 98.9% | 37.8% | 15.89 | 1.039 | 1.021 | 1.033 | 0.990 | 0.274 | 3/3 |
| ex1010 | 17 | 99.4% | 34.2% | 10.57 | 0.563 | 1.011 | 1.020 | 0.971 | 0.311 | 3/3 |
| ch_intrinsics | 12 | 94.6% | 12.5% | 7.64 | 1.008 | 1.000 | 0.988 | 0.990 | 0.340 | 3/3 |
| diffeq1 | 13 | 92.3% | 0.0% | 10.56 | 0.000 | 0.974 | 0.993 | 0.996 | 0.374 | 3/3 |
| blob_merge | 31 | 99.1% | 58.2% | 17.42 | 2.435 | 1.001 | 0.996 | 1.060 | 0.319 | 3/3 |
| or1200 | 39 | 97.4% | 0.0% | 16.63 | 0.000 | 1.001 | 0.996 | 1.023 | 0.348 | 3/3 |
| stereovision2 | 68 | 99.5% | 54.2% | 37.40 | 2.522 | 1.013 | 1.009 | 0.983 | 0.317 | 3/3 |
| bgm | 136 | 99.1% | 64.9% | 28.84 | 1.714 | 1.006 | 1.003 | 0.993 | 0.333 | 3/3 |
| LU8PEEng | 105 | 99.3% | 57.7% | 30.04 | 1.682 | 1.002 | 1.003 | 1.007 | 0.304 | 3/3 |
| **Mean** | 5% | 97.5% | 31.9% | 18.38 | 1.096 | 1.003 | 1.002 | 1.000 | 0.325 | |

- **Routing from scratch kept 39% to 54% of the routes on average**, and as few as 8% on the
  larger MCNC circuits: on VPR's graphs, small changes anywhere move routes everywhere.
- **Incremental routing kept 99% of the routes with 1% of the nets changed, and 94% with 5%**,
  and 94% and 84% with 1% and 5% of the blocks moved. Every result was legal, as from scratch,
  for wire 0.2% to 0.9% longer and a critical path as long within 0.2%, in 0.4% to 7% of the
  expansions. The time is 20% to 31% of routing from scratch, more than the expansions say,
  because both set up the same timing analysis, which takes most of the time once there is
  little to route.
- **ex1010, clma, blob_merge and bgm keep the fewest**, 59% to 74% with 5% of the blocks moved,
  and they are also those whose routes from scratch change most.
- **Incremental placement moved 20% and 32% of the blocks**, by 0.8 and 1.1 tiles on average,
  where placing from scratch moved 96% and 98% of them, by 18 tiles. Its bounding-box estimate
  was 0.2% and 0.3% above, and its routes had as much wire, within 1%, and as long a critical
  path, in a third of the time.

### What this does not show

- **Real engineering changes.** The changes are synthetic: random nets added, removed or given a
  sink, random nearby blocks swapped, random blocks or nets taken out and put back. Real changes
  are clustered, in one part of a design, which should be easier to keep local.
- **FPGA placement as `fpga::place_design` does it.** Its timing and congestion terms are left
  out on both sides, as `place::restrict` works on the generic problem. `place_incremental` can
  take any `CostTerm`, so they could be added.
- **Changes to the device**, on VTR's circuits: blocked resources are only measured on grids.
- **Larger changes.** At 5%, ex1010 already loses 40% of its routes. How far beyond that seeds
  still pay is not measured.
- **The quality of the placement before.** The placements before were of the netlist without
  the change, so the incremental placement starts from one that never saw it. Where nets were
  added, that is all the wirelength it loses on the generated problems.

## PCB placement and routing in a loop

M9 on PCBs: the [curated boards](../testdata/kicad/README.md) placed from nothing, as in [PCB
placement of the curated boards](#pcb-placement-of-the-curated-boards), routed, and then placed
again where routing found trouble and rerouted incrementally, round after round while that
pays, by `kicad::place_and_route`; against the one-pass flow it starts from, and against the
boards as their designers placed them. Then engineering changes on the boards as placed by
hand, rerouted by `pcb::eco::reroute`, and pin swaps. Measured on 2026-11-11 with rustc 1.97.1,
on the AMD Ryzen Threadripper PRO 9995WX, shared with other work, one thread a board, the
boards side by side, with the default options, on commit `c0a93fd`.

### How

```text
GUIDE_LOOP=1 GUIDE_KICAD=1 cargo test --release --features kicad curated_boards_loop -- --nocapture
GUIDE_ECO=1 cargo test --release --features kicad curated_boards_eco -- --nocapture
GUIDE_SWAP=1 GUIDE_KICAD=1 cargo test --release --features kicad curated_boards_swap -- --nocapture
scripts/loop-boards.sh GUIDE_LOOP curated_boards_loop BOARD...   # a process for each board
```

[`flow_tests.rs`](../src/kicad/flow_tests.rs) runs the loop on each board and checks both
boards it returns, the one-pass flow's and the loop's, as written, read back: our DRC must find
no violation involving a routed item or a pad of a part that moved, beyond what the board has
before routing, and KiCad 10.0.3's DRC at error severity must find none of those that it
doesn't find on the board as placed by hand and routed alike, as in the placement benchmark,
courtyards, silkscreen and thermal reliefs included, and no connection missing.

The loop, round by round:

1. **Feedback.** Each net that no zone connects is charged what it costs beyond the
   half-perimeter of its pads: 3 mm a via (`FlowOptions::via_mm`), its track beyond its
   half-perimeter, and much more if it failed. Each movable part is charged its nets' charges;
   the dearest 40% of the parts, by count, are the window, and the share halves after each
   round that keeps nothing. The window's nets weigh 100% to 400% of what they did, by their
   charge against their half-perimeter, and congestion is priced from a map of what routing
   used, each track's length spread along it and a via as 3 mm, against half of what the layers
   carry, in place of the RUDY estimate.
2. **Sides.** A window's part whose connections, by its nets of 12 pins at most, are mostly to
   parts on the other side alone goes there.
3. **Placing again**, by `pcb::place::refine_components`: the window's parts are legalized from
   where they are, among the others, fixed and taking their room, then annealed at a low
   temperature, within 5 mm of where they and their nets' pins are (`FlowOptions::reach`). If
   the parts sent to the other side don't fit there, the round is placed again without.
4. **Routing again**, by `pcb::eco::reroute`: the nets of the parts that moved lose their
   copper, but those with zones, which lose only their stubs where parts were and are; copper
   that the parts' new places make a violation of goes too; the rest is fixed copper, and only
   what it leaves apart is routed.
5. **Thermal reliefs.** Routes that crowd a pad of a plane's net can leave its relief one spoke,
   which KiCad reports as `starved_thermal`. Such reliefs are counted from the fills, and
   `pcb::eco::relieve` keeps a place for a second spoke free, as an obstacle, and reroutes the
   nets that crossed it, twice at most, if that pays.
6. **Judging**: a round is kept if it leaves fewer connections missing, or as many and fewer
   starved reliefs, or as many of both and less track plus 3 mm a via. The loop stops after 6
   rounds, or 3 in a row that keep nothing.

### Results

Connections, vias and track, in millimetres, over the whole board as written: every track and
via, stubs to zones included, which is why the numbers by hand are a little above those of
the placement benchmark's table. Seconds are placing and routing, the loop's including the
one pass it starts from. "Moved" is how many parts the loop's placement has elsewhere than the
one pass's, and how far on average and at most, by the Manhattan distance of their origins:
a part turned or flipped in place counts as moved by 0 mm. KiCad's column has the violations
that count and the missing connections, for the one pass, then the loop.

| Board | By hand | One pass | Loop | Rounds kept of run | Moved: parts, mean, most, mm | Seconds: one pass, loop | Our DRC | KiCad |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `GB-BRK-LINK-A` | 8/8, 0, 93 | 8/8, 0, 93 | 8/8, 0, 93 | 0 of 0 | 0 | 0.1, 0.1 | 0 | 0, 0; 0, 0 |
| `TS4231-BOB` | 13/13, 0, 31 | 13/13, 11, 71 | 13/13, **8**, **64** | 1 of 1 | 1, 7.8, 7.8 | 0.6, 5.7 | 0 | 0, 0; 0, 0 |
| `lisn` | 60/60, 7, 144 | 60/60, 14, 253 | 60/60, **13**, 255 | 1 of 4 | 3, 3.1, 5.3 | 0.7, 21.7 | 0 | 0, 0; 0, 0 |
| `isi-channel` | 29/29, 0, 453 | 29/29, 0, 453 | 29/29, 0, 453 | 0 of 0 | 0 | 0.2, 0.2 | 0 | 0, 0; 0, 0 |
| `placebo` | 50/50, 9, 292 | 50/50, 17, 257 | 50/50, 17, 257 | 0 of 3 | 0 | 1.0, 2.7 | 0 | 0, 0; 0, 0 |
| `udb-s` | 33/33, 11, 149 | 33/33, 16, 143 | 33/33, **13**, **125** | 1 of 1 | 1, 0, 0 | 2.1, 3.7 | 0 | 0, 0; 0, 0 |
| `flea` | 79/79, 52, 1607 | 79/79, 47, 1521 | 79/79, 47, 1521 | 0 of 0 | 0 | 1.6, 1.6 | 0 | 0, 0; 0, 0 |
| `GB-LIVE32` | 197/197, 165, 2283 | 197/197, 243, 2155 | 197/197, 243, 2155 | 0 of 2 | 0 | 82.5, 141.6 | 0 | 0, 0; 0, 0 |
| `nk3-nrf-mini` | 82/83, 55, 249 | 83/83, 63, 305 | 83/83, 63, 305 | 0 of 2 | 0 | 299.9, 333.4 | 0 | 0, 0; 0, 0 |
| `lpddr4-testbed` | 344/344, 220, 670 | 344/344, 219, 684 | 344/344, 219, **678** | 3 of 6 | 3, 0.9, 1.6 | 126.4, 138.9 | 0 | 0, 0; 0, 0 |
| `ddr5-testbed` | 255/255, 170, 876 | 255/255, 177, 932 | 255/255, **170**, **904** | 2 of 6 | 10, 3.1, 11.1 | 160.9, 217.0 | 0 | 0, 0; 0, 0 |

- **The loop gains where the one pass costs most against the hand placement, and never
  loses.** Of the nine boards with parts to move, it kept a round on five: fewer vias on
  four, as few as by hand on `ddr5-testbed`, and less track on four. Where nothing it tried
  was better, the one pass's board is the result, so the loop is never worse by its own
  measure. Completion was already full on every board but `nk3-nrf-mini` as placed by hand.
- **`TS4231-BOB`** loses 3 of its 11 vias and 7 mm of 71: the part that one pass sent to the
  back, whose connections are all on the front, comes back to the front, which with spacing
  has room for it after all. Packing the front crowds the ground plane's pads: an earlier run,
  with vias at 2 mm, kept a round of 7 vias only once a place for a second spoke had been freed
  at three thermal reliefs that routing had left one, which KiCad reports.
- **`udb-s`** loses 3 vias and 18 mm of 143, by turning one part in place.
- **`ddr5-testbed`** puts four parts on the other side, where most of their connections are,
  and moves ten by 3 mm on average, for 7 vias and 28 mm less; `lpddr4-testbed` gains 6 mm.
- **`GB-LIVE32`, `placebo` and `nk3-nrf-mini`** keep nothing. On `GB-LIVE32`, rounds moving 5
  and 6 parts gave 239 vias for 33 mm more track, and 246; on `placebo`, 19 to 21 vias for 5
  to 9 mm less; on `nk3-nrf-mini`, rounds that put two parts and one on the other side gave 68
  and 64 vias against 63. Routing incrementally around everything else takes the moved parts' nets where there is
  room, which is rarely where fewer vias are: the hand placements' 78 fewer vias on `GB-LIVE32`
  come from where its 44 parts are, packed by function, which windows of a few parts at a time
  don't reach.
- **Time.** A round routes again only what moved: 0.5 to 16 seconds on the small boards, the
  longest where starved reliefs were routed again, 1.3 to 44 on the DRAM boards, 12 and 21 on
  `nk3-nrf-mini` and 2 and 57 on `GB-LIVE32`, against 83 to 300 seconds for the first routing
  of those boards. The loop adds 10% (`lpddr4-testbed`, `nk3-nrf-mini`) to 72% (`GB-LIVE32`) to
  the one pass on the large boards; on the small ones, where the one pass takes a second or
  two, it takes up to 30 times as long, 22 seconds at most.
- **Sides, in the one pass too.** Parts sent to the other side are now those whose connections
  hold them least to theirs, two-pin parts first: `udb-s`'s one pass went from 13 vias and
  162 mm in the placement benchmark to 16 and 143; `TS4231-BOB`'s and `nk3-nrf-mini`'s are
  unchanged. Ordering them by how many vias they would need, before size, sent larger parts
  to the back on `nk3-nrf-mini`, for 22% more half-perimeter and a routing that ran for over
  40 minutes before it was stopped; so only whether anything holds a part to its side comes
  before its size.

### Engineering changes on the boards

Each board as placed by hand, routed by `pcb::route`, then changed, and routed again two ways:
incrementally by `pcb::eco::reroute`, from the routing before, and from nothing by
`pcb::route`. The changes, those each board has room and nets for:

- **nothing**: the same board, which must keep every track and via;
- **a part turned**: the first movable part of two pins, both on nets that routing used, turned
  half a turn, so that its pads change places;
- **a part moved**: the movable part of most pins, up to 16, moved 1 mm in the first of eight
  directions where its courtyard overlaps nothing new and stays clear of the edge;
- **a net removed**: the first routed net of two pads with no zone, whose pads lose their net;
- **a net added**: the two nearest pads, on different parts and the outer layers, of nets of
  one pad each, joined by a new net;
- **a part added**: two pads of 0.6 by 0.5 mm, 1 mm apart, on the front, between the first
  routed signal net of two pads or more and ground, where they clear every courtyard, pad,
  mask opening and keepout, nearest the net's first pad.

"Copper changed" is the length of the tracks of the routing before that the routing after
doesn't have, exactly, and of those it has that the routing before doesn't, as a fraction of
the track before, with the vias removed and added. Both routings are checked by our DRC, which
must find nothing wrong with routed copper, and for connectivity. Connections are over the
whole board.

| Board | Change | Incrementally: connections, vias, track, s | Copper changed | From nothing: connections, vias, track, s | Copper changed |
| --- | --- | --- | --- | --- | --- |
| `TS4231-BOB` | nothing | 13/13, 0, 31, 0.14 | 0% (0 vias) | 13/13, 0, 31, 0.12 | 0% (0 vias) |
| `TS4231-BOB` | D1 turned 180° | 13/13, 2, 33, 0.15 | 65% (2 vias) | 13/13, 2, 33, 0.13 | 74% (2 vias) |
| `TS4231-BOB` | U1 moved 1 mm | 13/13, 0, 31, 0.15 | 147% (0 vias) | 13/13, 0, 31, 0.12 | 147% (0 vias) |
| `TS4231-BOB` | net Net-(U1-AVDD) removed | 12/12, 0, 29, 0.14 | 5% (0 vias) | 12/12, 0, 29, 0.12 | 5% (0 vias) |
| `TS4231-BOB` | part added, +3.3V to GND | 15/15, 2, 37, 0.41 | 33% (2 vias) | 15/15, 2, 37, 0.20 | 33% (2 vias) |
| `lisn` | nothing | 60/60, 7, 144, 0.65 | 0% (0 vias) | 60/60, 7, 144, 0.52 | 0% (0 vias) |
| `lisn` | R2 turned 180° | 60/60, 9, 151, 0.64 | 21% (2 vias) | 60/60, 9, 151, 0.52 | 21% (2 vias) |
| `lisn` | T1 moved 1 mm | 60/60, 7, 140, 1.43 | 73% (8 vias) | 60/60, 7, 140, 0.65 | 73% (8 vias) |
| `lisn` | net /DIFF_MODE removed | 59/59, 7, 125, 0.66 | 13% (0 vias) | 59/59, 7, 125, 0.52 | 13% (0 vias) |
| `lisn` | part added, /OUTPUT to GND | 62/62, 7, 144, 0.66 | 49% (4 vias) | 61/62, 7, 147, 0.52 | 14% (0 vias) |
| `placebo` | nothing | 50/50, 9, 292, 0.36 | 0% (0 vias) | 50/50, 9, 292, 0.42 | 0% (0 vias) |
| `placebo` | R4 turned 180° | 50/50, 9, 293, 0.38 | 15% (0 vias) | 50/50, 9, 293, 0.44 | 15% (0 vias) |
| `placebo` | C4 moved 1 mm | 50/50, 9, 293, 0.41 | 11% (0 vias) | 50/50, 10, 292, 0.44 | 22% (1 vias) |
| `placebo` | net /PA2 removed | 49/49, 9, 282, 0.37 | 4% (0 vias) | 49/49, 9, 280, 0.45 | 14% (2 vias) |
| `udb-s` | nothing | 33/33, 11, 149, 0.48 | 0% (0 vias) | 33/33, 11, 149, 1.23 | 0% (0 vias) |
| `udb-s` | F1 turned 180° | 33/33, 11, 143, 0.51 | 39% (0 vias) | 33/33, 12, 148, 1.32 | 23% (1 vias) |
| `udb-s` | U1 moved 1 mm | 33/33, 13, 161, 0.57 | 83% (12 vias) | 33/33, 13, 161, 1.14 | 83% (12 vias) |
| `udb-s` | net Net-(J1-PadA5) removed | 32/32, 9, 139, 0.47 | 7% (2 vias) | 32/32, 8, 126, 1.39 | 85% (11 vias) |
| `udb-s` | part added, VBUS to GND | 35/35, 11, 141, 0.52 | 47% (0 vias) | 35/35, 11, 152, 1.56 | 32% (0 vias) |
| `flea` | nothing | 79/79, 52, 1607, 0.01 | 0% (0 vias) | 79/79, 52, 1607, 5.85 | 0% (0 vias) |
| `flea` | net unconnected-(PSW1-Pad3) removed | 78/78, 51, 1601, 0.01 | 0% (1 vias) | 78/78, 52, 1584, 5.29 | 31% (26 vias) |
| `flea` | part added, unconnected-(PSW1-Pad3) to GND | 81/81, 58, 1635, 7.35 | 29% (20 vias) | 81/81, 52, 1653, 5.98 | 10% (12 vias) |
| `GB-LIVE32` | nothing | 197/197, 165, 2283, 0.39 | 0% (0 vias) | 197/197, 165, 2283, 29.95 | 0% (0 vias) |
| `GB-LIVE32` | R4 turned 180° | 197/197, 170, 2267, 0.72 | 10% (17 vias) | 197/197, 165, 2290, 29.07 | 1% (0 vias) |
| `GB-LIVE32` | U1 moved 1 mm | 197/197, 165, 2281, 0.53 | 4% (0 vias) | 197/197, 165, 2283, 25.19 | 1% (0 vias) |
| `GB-LIVE32` | net Net-(D1-Pad1) removed | 196/196, 165, 2281, 0.39 | 0% (0 vias) | 196/196, 165, 2281, 23.84 | 0% (0 vias) |
| `GB-LIVE32` | net added, pad 4 of J2 to pad 34 of U2 | 197/198, 165, 2283, 7.40 | 0% (0 vias) | 197/198, 165, 2283, 25.12 | 0% (0 vias) |
| `GB-LIVE32` | part added, Net-(C1-Pad2) to GND | 199/199, 166, 2283, 0.45 | 2% (5 vias) | 199/199, 163, 2306, 26.46 | 3% (6 vias) |
| `nk3-nrf-mini` | nothing | 82/83, 55, 249, 0.26 | 0% (0 vias) | 82/83, 55, 249, 20.83 | 0% (0 vias) |
| `nk3-nrf-mini` | U3 moved 1 mm | 78/83, 50, 238, 111.43 | 53% (27 vias) | 77/83, 53, 233, 52.94 | 174% (96 vias) |
| `nk3-nrf-mini` | net Net-(C3-Pad2) removed | 81/82, 55, 248, 0.27 | 0% (0 vias) | 81/82, 55, 248, 23.35 | 0% (0 vias) |
| `nk3-nrf-mini` | net added, pad B3 of U1 to pad 5 of D2 | 82/84, 55, 249, 21.25 | 0% (0 vias) | 82/84, 55, 263, 36.72 | 143% (58 vias) |
| `lpddr4-testbed` | nothing | 344/344, 220, 670, 1.25 | 0% (0 vias) | 344/344, 220, 670, 113.38 | 0% (0 vias) |
| `lpddr4-testbed` | R2 turned 180° | 344/344, 220, 668, 1.21 | 7% (2 vias) | 344/344, 193, 721, 95.80 | 33% (49 vias) |
| `lpddr4-testbed` | IC1 moved 1 mm | 344/344, 220, 671, 1.95 | 30% (12 vias) | 344/344, 219, 677, 102.95 | 25% (13 vias) |
| `lpddr4-testbed` | net Net-(IC1-P0) removed | 343/343, 218, 660, 1.08 | 2% (2 vias) | 343/343, 218, 660, 108.02 | 2% (2 vias) |
| `lpddr4-testbed` | net added, pad 5 of U2 to pad 13 of IC1 | 344/345, 217, 675, 4.78 | 11% (7 vias) | 344/345, 220, 670, 102.17 | 0% (0 vias) |
| `lpddr4-testbed` | part added, Net-(IC1-P0) to GND | 346/346, 220, 677, 1.30 | 3% (0 vias) | 346/346, 221, 673, 99.90 | 5% (5 vias) |
| `ddr5-testbed` | nothing | 255/255, 170, 876, 0.91 | 0% (0 vias) | 255/255, 170, 876, 164.70 | 0% (0 vias) |
| `ddr5-testbed` | R12 turned 180° | 255/255, 170, 877, 1.06 | 4% (4 vias) | 255/255, 170, 877, 159.11 | 4% (4 vias) |
| `ddr5-testbed` | IC1 moved 1 mm | 255/255, 170, 880, 1.59 | 19% (16 vias) | 255/255, 169, 885, 153.99 | 17% (17 vias) |
| `ddr5-testbed` | net RESET_N removed | 254/254, 168, 866, 0.85 | 1% (2 vias) | 254/254, 153, 890, 154.13 | 142% (183 vias) |
| `ddr5-testbed` | net added, pad 5 of U2 to pad 13 of IC1 | 255/256, 167, 881, 4.11 | 6% (7 vias) | 255/256, 170, 876, 153.97 | 0% (0 vias) |
| `ddr5-testbed` | part added, RESET_N to GND | 257/257, 168, 887, 1.09 | 18% (18 vias) | 253/257, 153, 801, 198.10 | 128% (169 vias) |

- **Nothing changed keeps everything**, every track and via on every board, in 0.01 to 1.3
  seconds. Routing from scratch is deterministic, so it gives the same board too, but in its
  full time, up to 165 seconds.
- **Incrementally, a change costs about the copper it touches.** Removing a net changed 0% to
  13% of the track, adding a net 0% to 11%, adding a part 2% to 49%, and turning or moving a
  part 4% to 83%, or 147% on `TS4231-BOB`, whose `U1` has every net.
- **From scratch, one change can move most of a dense board.** On the small boards, nets route
  one at a time in an order that a change hardly moves, so routing from scratch changes little
  more than the change. But it changed 142% of `ddr5-testbed`'s track for a net removed and
  128% for a part added, 174% and 143% of `nk3-nrf-mini`'s for a part moved and a net added,
  85% of `udb-s`'s and 31% of `flea`'s for a net removed, and 33% of `lpddr4-testbed`'s, with
  49 vias, for a resistor turned; incrementally, those were 0% to 53%.
- **Quality is the same, within a few vias, and completion as good or better.** Incrementally,
  no change left more connections out than from scratch did, and three left fewer: 4 on
  `ddr5-testbed`'s added part, 1 on `lisn`'s, 1 on `nk3-nrf-mini`'s moved `U3`, which leaves 5
  out either way. Vias and track went either way: `GB-LIVE32`'s turned resistor took 5 vias
  more, `flea`'s added part 6 more, while from scratch `ddr5-testbed` found 15 fewer vias and
  24 mm more track for its removed net, and `lpddr4-testbed` 27 fewer for 53 mm more.
- **Time.** On the large boards, a change took 0.3 to 8 seconds incrementally, where routing
  from scratch took 24 to 30 seconds on `GB-LIVE32`, 21 to 53 on `nk3-nrf-mini` and 96 to 198
  on the DRAM boards; but moving `nk3-nrf-mini`'s `U3`, which leaves connections out, took 111
  seconds, as the area taken out is widened twice around them. On the small boards it costs
  about what routing from scratch does, which there is mostly filling zones and checking.

### Pin swaps

Equivalent pins are the caller's to give, as `pcb::swap::SwapGroup`s, as a board file says
nothing of them; `kicad::swap_groups` gives the safe automatic ones, the two pins of
resistors, capacitors other than polarized ones, inductors and ferrite beads. Swaps are made
where they shorten the nets, two-pin nets by the distance between their pins, which uncrosses
them, others by half-perimeter, with nets that zones connect left out, and written back with
`kicad::replace_pad_nets`, with a list of the pads changed for the schematic.

**Passives**, after the one pass's placement, by the half-perimeter of the nets that count:

| Board | Two-pin passives | Swapped | Half-perimeter, mm: placed → swapped |
| --- | --- | --- | --- |
| `TS4231-BOB` | 3 | 0 | 61.5 → 61.5 |
| `lisn` | 20 | 0 | 156.7 → 156.7 |
| `placebo` | 9 | 2 | 251.5 → 252.9 |
| `udb-s` | 3 | 1 | 104.4 → 103.4 |
| `GB-LIVE32` | 29 | 0 | 1899.7 → 1899.7 |
| `nk3-nrf-mini` | 17 | 0 | 156.9 → 156.9 |
| `lpddr4-testbed` | 54 | 0 | 522.8 → 522.8 |
| `ddr5-testbed` | 50 | 1 | 760.9 → 760.8 |

Placement already turns parts by half turns, which is the same as swapping a passive's pins,
so few swaps are left to make: 4 on three boards. `placebo`'s two shorten their nets' distances
but not their half-perimeters. These were not routed.

**DQ bits within byte lanes**, on the DRAM boards as placed by hand: the bits of each byte lane
on the DRAM, by the names of their nets (`DQ0` to `DQ7` on `ddr5-testbed`, `DQ00_A` to
`DQ15_A`, two lanes, on `lpddr4-testbed`), swapped for length, from the designers' assignment,
and from that assignment reversed within each lane, as a netlist drawn without the layout in
mind might have it. Each routed with the default options.

| Board, bits | DQ bits | Pads changed | Connections | Vias | Track, mm | Routing, s | Our DRC | KiCad |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `lpddr4-testbed`, as designed | 16 | 0 | 344/344 | 220 | 670 | 105.1 | 0 | 0 violations, 0 unconnected |
| `lpddr4-testbed`, as designed, swapped | 16 | 8 | 344/344 | 224 | 676 | 131.6 | 0 | 0 violations, 0 unconnected |
| `lpddr4-testbed`, reversed | 16 | 16 | 344/344 | 221 | 762 | 163.1 | 0 | 0 violations, 0 unconnected |
| `lpddr4-testbed`, reversed, swapped | 16 | 14 | 344/344 | 227 | 771 | 148.4 | 0 | 0 violations, 0 unconnected |
| `ddr5-testbed`, as designed | 8 | 0 | 255/255 | 170 | 876 | 160.1 | 0 | 0 violations, 0 unconnected |
| `ddr5-testbed`, as designed, swapped | 8 | 4 | 249/255 | 147 | 845 | 459.4 | 0 | 0 violations, 6 unconnected |
| `ddr5-testbed`, reversed | 8 | 8 | 255/255 | 175 | 902 | 290.6 | 0 | 0 violations, 0 unconnected |
| `ddr5-testbed`, reversed, swapped | 8 | 7 | 255/255 | 171 | 878 | 357.8 | 0 | 0 violations, 0 unconnected |

- **On `ddr5-testbed`, swaps undo a careless assignment**: reversed, its lane costs 5 vias and
  26 mm more than as designed; swapped for length, it comes back to within a via and 2 mm of
  the designers' assignment.
- **Swapping the designers' own assignment for length doesn't pay.** It rotates four bits on
  each board, and `ddr5-testbed` then routes 6 connections short, in three times the time,
  where `lpddr4-testbed` takes 4 vias more. The designers' order follows how the balls escape
  the BGA and reach the edge connector's pins on the layers available, which the distance
  between two pins doesn't see.
- **On `lpddr4-testbed`, length is the wrong measure either way**: from the reversed lanes,
  swaps for length made it 6 vias and 9 mm worse. A swap that pays would have to be judged by
  routing, which the loop does for placement: swaps are kept out of its default, and given as
  `FlowOptions::swaps` they are made once, after placement.

### What this does not show

- **Gains are modest, and on the boards with most to gain, nothing.** The loop's windows are
  a few parts, near where they were; what makes `GB-LIVE32`'s hand placement 78 vias cheaper is
  where everything is. Pricing vias in placement itself, beyond sides, is not done.
- **One seed.** Each round anneals with the next seed, but the loop was run once a board.
  Results on the hard boards are chaotic, as [above](#what-changes-what): a round that moves one
  part can change a hundred vias on `nk3-nrf-mini`.
- **Thermal reliefs** are counted by our fill's own spokes, which reach the fill where KiCad's
  do, but are not KiCad's count: they found one relief starved on `TS4231-BOB`'s one pass that
  KiCad's check didn't count, and three on a round where KiCad found two.
- **Changes are generated**, one at a time, by rules that pick the first part or net that
  qualifies, not by a designer.
- **Pin swaps are judged by length**, not by routing, and only DQ bits and passives were
  tried: no connector pins, resistor networks or pin multiplexers, which the API takes but no
  curated board says.

## Multi-agent path finding on the MovingAI benchmarks

The first domain outside electronics (roadmap M10): `guide::mapf` plans agents one at a time
against the others' plans at a price, and raises the price until nobody collides, the engine's
negotiation. It does so over safe intervals by default, and through a time-expanded space that
the engine negotiates over unchanged, the way of the first results, as an option. [The domain
notes](domains.md#multi-agent-path-finding) say how the problem maps onto the core.

Measured on 2026-11-17 with rustc 1.97.1, at `6ca112a`, on an AMD Ryzen Threadripper PRO 9995WX
shared with other work: a load of 120 to 150 from others' jobs throughout, so that times are
rough. The first results, through time, were measured on 2026-11-11 at `9c74967` and are kept
for comparison.

### How

- **The benchmarks** are the MovingAI MAPF benchmarks of Stern et al. (*Multi-Agent
  Pathfinding: Definitions, Variants, and Benchmarks*, SoCS 2019), from
  <https://movingai.com/benchmarks/mapf.html>, under the Open Data Commons Attribution
  License. They are not copied here: [`scripts/mapf/fetch.sh`](../scripts/mapf/fetch.sh)
  downloads them. All 33 maps, each with 25 random and 25 even scenarios.
- **Two sets of runs.** The six maps of the first results, `empty-32-32`, `random-32-32-20`,
  `room-32-32-4`, `maze-32-32-2`, `warehouse-10-20-10-2-1` and `den520d`, with their 25
  random scenarios and ten agent counts each, an instance being the first so many agents of a
  scenario, under one minute: 1500 instances, as before. Then every map, the 27 others with
  their first random scenario and all 33 with their first even one, at a quarter, a half,
  three quarters and all of the scenario's agents, under 30 seconds: 240 instances. The second
  set is small because it had to share the machine: at most two solver processes at once, each
  under 12 GB of address space and at the lowest priority. The first set's runs of `guide` had
  48 at a time, before that limit.
- **The problem** is the classic one: 4-connected moves, vertex and swap conflicts, agents
  staying on their goals, and the sum of costs, each agent's last arrival on its goal, as the
  objective.
- **The reference solvers** are built from source by
  [`scripts/mapf/build-reference.sh`](../scripts/mapf/build-reference.sh) and not copied here:
  CBSH2-RTC (Li et al.), an optimal solver, conflict-based search with its symmetry reasoning,
  at `a834df1`; EECBS (Li, Ruml and Koenig, AAAI 2021), bounded-suboptimal, with a bound of
  1.2, at `06ec705`, both under the University of Southern California's licence for research
  and non-profit use; and LaCAM3 (Okumura, 2024), an anytime solver that finds a solution
  fast and refines it for the rest of the time, at `1c3ecf8`, under the MIT licence, run on
  one thread (`--no-multi-thread`), as `guide` is. Its time is that to its first solution, and
  its sum of costs that of its last, when its time is up. In the first set, it was also run
  with its own threads on the first 5 scenarios of each map ("LaCAM3 threads"). LaCAM3 reports
  two costs: `soc`, the sum of costs as everyone else has it, which is the one shown, and
  `sum_of_loss`, which it minimizes. In the second set, CBSH2-RTC and EECBS ran on the random
  scenarios only, each scenario's counts in increasing order, and once one failed, the larger
  ones were counted as failed without running them; their run on the even scenarios was lost
  to a bug in the script, since fixed, and there was no time to run it again.
- **`guide`'s time** includes working out the distance from every vertex to every goal, which
  the first results did not count, and is given twice: to its first solution, LaCAM's, which it
  falls back on if negotiation doesn't settle, and to the solution it returns. Its limit sets
  the engine's cancel flag from a timer: without it, its results do not depend on the machine.
- **Sums of costs** are compared with the sum of the agents' shortest paths, each alone, which
  is a lower bound on any solution; with CBSH2-RTC's optimum where it found one; and with the
  MAPF tracker's best known solutions and lower bounds (Shen et al., *Tracking Progress in
  Multi-Agent Path Finding*, 2023, <https://tracker.pathfinding.ai>), which
  [`scripts/mapf/tracker.py`](../scripts/mapf/tracker.py) downloads from the API the
  tracker's own pages read, for every scenario and agent count. Every solution of `guide`'s is
  checked by `mapf::check`, which knows nothing of the planner, the space or the engine.
- **`guide`'s options** are `SolveOptions::default()`: safe intervals, `mapf::engine_options()`
  for the schedule, which is the engine's defaults with a history factor of 0.5 (0.25 a move
  over safe intervals, where a move costs 1), up to 100 iterations, a first solution by LaCAM,
  and 10 passes of shortening.

```text
scripts/mapf/fetch.sh DATA                                  # about 18 MB
scripts/mapf/build-reference.sh SOLVERS                     # needs cmake and Boost
python3 scripts/mapf/tracker.py DATA                        # about 150 MB of best known results
JOBS=24 scripts/mapf/guide.sh DATA OUT                      # the six maps
JOBS=48 python3 scripts/mapf/reference.py DATA OUT SOLVERS  # about 3 hours
ONLY=lacam3-mt SCENS=5 JOBS=12 \
  python3 scripts/mapf/reference.py DATA OUT SOLVERS        # LaCAM3 with its threads
python3 scripts/mapf/tables.py OUT                          # the six maps' tables
export JOBS=2 SCENS=1 PARTS=4 LIMIT=30                      # every map, as run here
scripts/mapf/guide.sh DATA OUT2 MAP...                      # the 27 other maps
KIND=even scripts/mapf/guide.sh DATA OUT2 all
python3 scripts/mapf/reference.py DATA OUT2 SOLVERS MAP...
KIND=even ONLY=lacam3 python3 scripts/mapf/reference.py DATA OUT2 SOLVERS all
python3 scripts/mapf/tables.py --summary OUT2 DATA          # the summary tables
```

### Results: the six maps

- **Success.** `guide` solved all 1500 instances within the minute, where the first results
  left two of warehouse unsolved. LaCAM3 solved all 1500, EECBS 1237 and CBSH2-RTC 308. None
  of `guide`'s fell back on its first solution: negotiation settled every time.
- **Sum of costs.** `guide`'s is as low as the first results' or lower at every map and
  agent count, by up to 2 points of the lower bound: 1.308 against 1.328 on empty-32-32 with
  500 agents, 1.279 against 1.300 on random-32-32-20 with 250, 1.231 against 1.250 on
  room-32-32-4 with 100, 1.069 against 1.078 on warehouse with 500. It is the lowest of the
  suboptimal solvers at every map and agent count, but for maze-32-32-2 with 100 agents, where EECBS solved one
  instance of 25, and LaCAM3 on its threads, which is slightly lower on room with 20 and 30
  agents and warehouse with 50, on 5 scenarios against 25. Where CBSH2-RTC proved an optimum,
  `guide` is within 0% to 1.4% of it, from 1.7% before; LaCAM3 0.1% to 6.3% and EECBS 0.1%
  to 4.0%.
- **Time to a first solution.** LaCAM's first search, which `guide` now starts with, took at
  most 11 ms in median on empty-32-32 and 287 ms on den520d with 1000 agents, the distances to
  every goal included, where LaCAM3 took 374 ms and 7.7 s to its own first solution. Its sums
  of costs were 37% above negotiation's over the 1500 instances, and up to twice as high: a
  legal answer at once, not a good one.
- **Time to the solution returned.** Negotiation over safe intervals is faster than through
  time on the maps where waits are long: warehouse with 500 agents in a median of 2.4 s, from
  9.8 s, and maze-32-32-2 with 100 in 0.4 s, from 0.7 s. On the crowdest small maps it is
  about as fast as before, 1.8 s on empty-32-32 with 500 against 2.1 s, and LaCAM3 is still
  several times faster to its first solution there, 0.37 s, though not to its last, which is
  when its minute is up. These runs had 48 at a time on a machine loaded by others, and times
  are rough.

Times are medians over the instances each solved, in milliseconds; "guide first ms" is to
the first solution, LaCAM's. Sums of costs are aggregated over the instances each solved:
summed, then divided by the sum of the lower bounds or optima of the same instances.
"before" is the first results, through time.

#### empty-32-32

Solved within the limit, of each count's scenarios:

| Agents | guide | before | cbsh2-rtc | eecbs-1.2 | lacam3 | lacam3-mt |
| --- | --- | --- | --- | --- | --- | --- |
| 50 | 25/25 | 25/25 | 25/25 | 25/25 | 25/25 | 5/5 |
| 100 | 25/25 | 25/25 | 23/25 | 25/25 | 25/25 | 5/5 |
| 150 | 25/25 | 25/25 | 1/25 | 25/25 | 25/25 | 5/5 |
| 200 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 250 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 300 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 350 | 25/25 | 25/25 | 0/25 | 14/25 | 25/25 | 5/5 |
| 400 | 25/25 | 25/25 | 0/25 | 0/25 | 25/25 | 5/5 |
| 450 | 25/25 | 25/25 | 0/25 | 0/25 | 25/25 | 5/5 |
| 500 | 25/25 | 25/25 | 0/25 | 0/25 | 25/25 | 5/5 |

Sum of costs over the lower bound, on the scenarios that each solved, then over CBSH2-RTC's optimum on those that CBSH2-RTC solved too, and median milliseconds to a solution, LaCAM3's first:

| Agents | guide / LB | before / LB | LaCAM3 / LB | LaCAM3 threads / LB | EECBS / LB | guide / opt | LaCAM3 / opt | EECBS / opt | guide ms | guide first ms | before ms | LaCAM3 first ms | EECBS ms | CBSH2 ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 50 | 1.002 | 1.002 | 1.007 | 1.002 | 1.004 | 1.000 | 1.005 | 1.003 | 3 | 1 | 2 | 6 | 4 | 5 |
| 100 | 1.005 | 1.006 | 1.022 | 1.006 | 1.012 | 1.001 | 1.018 | 1.008 | 11 | 2 | 7 | 13 | 14 | 120 |
| 150 | 1.011 | 1.013 | 1.040 | 1.013 | 1.031 | 1.003 | 1.031 | 1.019 | 24 | 3 | 18 | 24 | 30 | 7810 |
| 200 | 1.022 | 1.025 | 1.069 | 1.030 | 1.061 | - | - | - | 51 | 4 | 38 | 38 | 60 | - |
| 250 | 1.039 | 1.043 | 1.110 | 1.052 | 1.105 | - | - | - | 97 | 5 | 76 | 57 | 129 | - |
| 300 | 1.069 | 1.073 | 1.192 | 1.107 | 1.170 | - | - | - | 197 | 6 | 185 | 96 | 309 | - |
| 350 | 1.105 | 1.115 | 1.300 | 1.170 | 1.191 | - | - | - | 352 | 7 | 399 | 165 | 11188 | - |
| 400 | 1.155 | 1.168 | 1.490 | 1.274 | - | - | - | - | 602 | 8 | 680 | 204 | - | - |
| 450 | 1.227 | 1.244 | 1.677 | 1.383 | - | - | - | - | 1067 | 9 | 1212 | 287 | - | - |
| 500 | 1.308 | 1.328 | 1.852 | 1.551 | - | - | - | - | 1823 | 11 | 2142 | 374 | - | - |

#### random-32-32-20

Solved within the limit, of each count's scenarios:

| Agents | guide | before | cbsh2-rtc | eecbs-1.2 | lacam3 | lacam3-mt |
| --- | --- | --- | --- | --- | --- | --- |
| 25 | 25/25 | 25/25 | 25/25 | 25/25 | 25/25 | 5/5 |
| 50 | 25/25 | 25/25 | 24/25 | 25/25 | 25/25 | 5/5 |
| 75 | 25/25 | 25/25 | 1/25 | 25/25 | 25/25 | 5/5 |
| 100 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 125 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 150 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 175 | 25/25 | 25/25 | 0/25 | 21/25 | 25/25 | 5/5 |
| 200 | 25/25 | 25/25 | 0/25 | 3/25 | 25/25 | 5/5 |
| 225 | 25/25 | 25/25 | 0/25 | 0/25 | 25/25 | 5/5 |
| 250 | 25/25 | 25/25 | 0/25 | 0/25 | 25/25 | 5/5 |

Sum of costs over the lower bound, on the scenarios that each solved, then over CBSH2-RTC's optimum on those that CBSH2-RTC solved too, and median milliseconds to a solution, LaCAM3's first:

| Agents | guide / LB | before / LB | LaCAM3 / LB | LaCAM3 threads / LB | EECBS / LB | guide / opt | LaCAM3 / opt | EECBS / opt | guide ms | guide first ms | before ms | LaCAM3 first ms | EECBS ms | CBSH2 ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 25 | 1.011 | 1.011 | 1.026 | 1.017 | 1.016 | 1.002 | 1.017 | 1.007 | 2 | 1 | 1 | 3 | 2 | 3 |
| 50 | 1.027 | 1.028 | 1.056 | 1.037 | 1.041 | 1.005 | 1.033 | 1.018 | 6 | 1 | 5 | 7 | 7 | 436 |
| 75 | 1.048 | 1.049 | 1.091 | 1.067 | 1.074 | 1.006 | 1.037 | 1.016 | 16 | 2 | 16 | 13 | 17 | 46730 |
| 100 | 1.069 | 1.073 | 1.133 | 1.088 | 1.117 | - | - | - | 31 | 2 | 35 | 25 | 45 | - |
| 125 | 1.091 | 1.096 | 1.176 | 1.116 | 1.153 | - | - | - | 52 | 3 | 66 | 32 | 101 | - |
| 150 | 1.120 | 1.126 | 1.250 | 1.152 | 1.191 | - | - | - | 88 | 4 | 113 | 48 | 235 | - |
| 175 | 1.149 | 1.164 | 1.325 | 1.194 | 1.212 | - | - | - | 133 | 5 | 173 | 71 | 4160 | - |
| 200 | 1.190 | 1.205 | 1.427 | 1.252 | 1.204 | - | - | - | 194 | 6 | 285 | 90 | 12464 | - |
| 225 | 1.232 | 1.250 | 1.564 | 1.311 | - | - | - | - | 298 | 6 | 422 | 146 | - | - |
| 250 | 1.279 | 1.300 | 1.673 | 1.385 | - | - | - | - | 406 | 7 | 545 | 134 | - | - |

#### room-32-32-4

Solved within the limit, of each count's scenarios:

| Agents | guide | before | cbsh2-rtc | eecbs-1.2 | lacam3 | lacam3-mt |
| --- | --- | --- | --- | --- | --- | --- |
| 10 | 25/25 | 25/25 | 25/25 | 25/25 | 25/25 | 5/5 |
| 20 | 25/25 | 25/25 | 25/25 | 25/25 | 25/25 | 5/5 |
| 30 | 25/25 | 25/25 | 24/25 | 25/25 | 25/25 | 5/5 |
| 40 | 25/25 | 25/25 | 10/25 | 25/25 | 25/25 | 5/5 |
| 50 | 25/25 | 25/25 | 2/25 | 25/25 | 25/25 | 5/5 |
| 60 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 70 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 80 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 90 | 25/25 | 25/25 | 0/25 | 21/25 | 25/25 | 5/5 |
| 100 | 25/25 | 25/25 | 0/25 | 15/25 | 25/25 | 5/5 |

Sum of costs over the lower bound, on the scenarios that each solved, then over CBSH2-RTC's optimum on those that CBSH2-RTC solved too, and median milliseconds to a solution, LaCAM3's first:

| Agents | guide / LB | before / LB | LaCAM3 / LB | LaCAM3 threads / LB | EECBS / LB | guide / opt | LaCAM3 / opt | EECBS / opt | guide ms | guide first ms | before ms | LaCAM3 first ms | EECBS ms | CBSH2 ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10 | 1.019 | 1.020 | 1.031 | 1.031 | 1.022 | 1.003 | 1.015 | 1.006 | 0 | 0 | 1 | 1 | 0 | 1 |
| 20 | 1.032 | 1.033 | 1.055 | 1.026 | 1.043 | 1.004 | 1.027 | 1.015 | 2 | 0 | 2 | 2 | 2 | 7 |
| 30 | 1.060 | 1.063 | 1.100 | 1.050 | 1.081 | 1.008 | 1.047 | 1.028 | 5 | 1 | 7 | 4 | 8 | 754 |
| 40 | 1.083 | 1.091 | 1.147 | 1.089 | 1.114 | 1.008 | 1.063 | 1.040 | 9 | 1 | 15 | 6 | 19 | 7380 |
| 50 | 1.108 | 1.116 | 1.193 | 1.110 | 1.152 | 1.014 | 1.056 | 1.034 | 15 | 1 | 30 | 9 | 39 | 46046 |
| 60 | 1.131 | 1.144 | 1.233 | 1.155 | 1.179 | - | - | - | 25 | 1 | 46 | 13 | 64 | - |
| 70 | 1.156 | 1.169 | 1.281 | 1.184 | 1.218 | - | - | - | 36 | 2 | 59 | 15 | 98 | - |
| 80 | 1.178 | 1.195 | 1.323 | 1.208 | 1.234 | - | - | - | 52 | 2 | 77 | 20 | 174 | - |
| 90 | 1.211 | 1.228 | 1.368 | 1.236 | 1.245 | - | - | - | 66 | 2 | 118 | 25 | 2631 | - |
| 100 | 1.231 | 1.250 | 1.421 | 1.280 | 1.252 | - | - | - | 86 | 3 | 135 | 27 | 23720 | - |

#### maze-32-32-2

Solved within the limit, of each count's scenarios:

| Agents | guide | before | cbsh2-rtc | eecbs-1.2 | lacam3 | lacam3-mt |
| --- | --- | --- | --- | --- | --- | --- |
| 10 | 25/25 | 25/25 | 25/25 | 25/25 | 25/25 | 5/5 |
| 20 | 25/25 | 25/25 | 23/25 | 25/25 | 25/25 | 5/5 |
| 30 | 25/25 | 25/25 | 4/25 | 25/25 | 25/25 | 5/5 |
| 40 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 50 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 60 | 25/25 | 25/25 | 0/25 | 21/25 | 25/25 | 5/5 |
| 70 | 25/25 | 25/25 | 0/25 | 14/25 | 25/25 | 5/5 |
| 80 | 25/25 | 25/25 | 0/25 | 6/25 | 25/25 | 5/5 |
| 90 | 25/25 | 25/25 | 0/25 | 1/25 | 25/25 | 5/5 |
| 100 | 25/25 | 25/25 | 0/25 | 1/25 | 25/25 | 5/5 |

Sum of costs over the lower bound, on the scenarios that each solved, then over CBSH2-RTC's optimum on those that CBSH2-RTC solved too, and median milliseconds to a solution, LaCAM3's first:

| Agents | guide / LB | before / LB | LaCAM3 / LB | LaCAM3 threads / LB | EECBS / LB | guide / opt | LaCAM3 / opt | EECBS / opt | guide ms | guide first ms | before ms | LaCAM3 first ms | EECBS ms | CBSH2 ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10 | 1.010 | 1.011 | 1.023 | 1.024 | 1.013 | 1.001 | 1.013 | 1.003 | 1 | 0 | 2 | 2 | 1 | 6 |
| 20 | 1.033 | 1.035 | 1.055 | 1.040 | 1.046 | 1.004 | 1.023 | 1.017 | 7 | 1 | 6 | 4 | 8 | 578 |
| 30 | 1.060 | 1.062 | 1.101 | 1.072 | 1.089 | 1.004 | 1.038 | 1.028 | 19 | 1 | 31 | 8 | 53 | 38384 |
| 40 | 1.088 | 1.089 | 1.143 | 1.119 | 1.137 | - | - | - | 38 | 1 | 71 | 12 | 172 | - |
| 50 | 1.118 | 1.130 | 1.205 | 1.141 | 1.187 | - | - | - | 66 | 2 | 132 | 17 | 325 | - |
| 60 | 1.146 | 1.157 | 1.250 | 1.169 | 1.204 | - | - | - | 113 | 2 | 218 | 20 | 1213 | - |
| 70 | 1.170 | 1.188 | 1.306 | 1.235 | 1.215 | - | - | - | 164 | 2 | 289 | 26 | 4253 | - |
| 80 | 1.205 | 1.217 | 1.359 | 1.252 | 1.231 | - | - | - | 243 | 2 | 417 | 30 | 15493 | - |
| 90 | 1.227 | 1.245 | 1.407 | 1.299 | 1.230 | - | - | - | 335 | 3 | 539 | 37 | 31594 | - |
| 100 | 1.252 | 1.274 | 1.476 | 1.349 | 1.244 | - | - | - | 435 | 3 | 655 | 47 | 25871 | - |

#### warehouse-10-20-10-2-1

Solved within the limit, of each count's scenarios:

| Agents | guide | before | cbsh2-rtc | eecbs-1.2 | lacam3 | lacam3-mt |
| --- | --- | --- | --- | --- | --- | --- |
| 50 | 25/25 | 25/25 | 25/25 | 25/25 | 25/25 | 5/5 |
| 100 | 25/25 | 25/25 | 22/25 | 25/25 | 25/25 | 5/5 |
| 150 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 200 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 250 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 300 | 25/25 | 24/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 350 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 400 | 25/25 | 24/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 450 | 25/25 | 25/25 | 0/25 | 24/25 | 25/25 | 5/5 |
| 500 | 25/25 | 25/25 | 0/25 | 24/25 | 25/25 | 5/5 |

Sum of costs over the lower bound, on the scenarios that each solved, then over CBSH2-RTC's optimum on those that CBSH2-RTC solved too, and median milliseconds to a solution, LaCAM3's first:

| Agents | guide / LB | before / LB | LaCAM3 / LB | LaCAM3 threads / LB | EECBS / LB | guide / opt | LaCAM3 / opt | EECBS / opt | guide ms | guide first ms | before ms | LaCAM3 first ms | EECBS ms | CBSH2 ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 50 | 1.002 | 1.002 | 1.005 | 1.001 | 1.003 | 1.000 | 1.004 | 1.002 | 13 | 4 | 13 | 31 | 30 | 77 |
| 100 | 1.005 | 1.005 | 1.012 | 1.006 | 1.009 | 1.001 | 1.008 | 1.006 | 45 | 8 | 58 | 79 | 125 | 2716 |
| 150 | 1.011 | 1.012 | 1.030 | 1.021 | 1.019 | - | - | - | 109 | 12 | 184 | 152 | 300 | - |
| 200 | 1.018 | 1.019 | 1.046 | 1.031 | 1.030 | - | - | - | 207 | 17 | 478 | 203 | 588 | - |
| 250 | 1.025 | 1.026 | 1.074 | 1.049 | 1.043 | - | - | - | 356 | 21 | 864 | 320 | 1057 | - |
| 300 | 1.033 | 1.034 | 1.100 | 1.056 | 1.059 | - | - | - | 575 | 23 | 1482 | 403 | 2027 | - |
| 350 | 1.040 | 1.043 | 1.137 | 1.070 | 1.077 | - | - | - | 927 | 30 | 2565 | 560 | 3629 | - |
| 400 | 1.049 | 1.054 | 1.173 | 1.084 | 1.102 | - | - | - | 1401 | 36 | 4017 | 762 | 7387 | - |
| 450 | 1.060 | 1.067 | 1.220 | 1.105 | 1.122 | - | - | - | 1882 | 40 | 8305 | 863 | 11780 | - |
| 500 | 1.069 | 1.078 | 1.251 | 1.123 | 1.142 | - | - | - | 2402 | 42 | 9789 | 1034 | 22634 | - |

#### den520d

Solved within the limit, of each count's scenarios:

| Agents | guide | before | cbsh2-rtc | eecbs-1.2 | lacam3 | lacam3-mt |
| --- | --- | --- | --- | --- | --- | --- |
| 100 | 25/25 | 25/25 | 21/25 | 25/25 | 25/25 | 5/5 |
| 200 | 25/25 | 25/25 | 3/25 | 25/25 | 25/25 | 5/5 |
| 300 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 400 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 500 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 600 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 700 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 800 | 25/25 | 25/25 | 0/25 | 25/25 | 25/25 | 5/5 |
| 900 | 25/25 | 25/25 | 0/25 | 24/25 | 25/25 | 5/5 |
| 1000 | 25/25 | 25/25 | 0/25 | 23/25 | 25/25 | 5/5 |

Sum of costs over the lower bound, on the scenarios that each solved, then over CBSH2-RTC's optimum on those that CBSH2-RTC solved too, and median milliseconds to a solution, LaCAM3's first:

| Agents | guide / LB | before / LB | LaCAM3 / LB | LaCAM3 threads / LB | EECBS / LB | guide / opt | LaCAM3 / opt | EECBS / opt | guide ms | guide first ms | before ms | LaCAM3 first ms | EECBS ms | CBSH2 ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 100 | 1.001 | 1.001 | 1.001 | 1.001 | 1.001 | 1.000 | 1.001 | 1.001 | 132 | 43 | 83 | 279 | 189 | 834 |
| 200 | 1.001 | 1.002 | 1.003 | 1.002 | 1.003 | 1.000 | 1.001 | 1.001 | 421 | 80 | 296 | 615 | 714 | 5817 |
| 300 | 1.002 | 1.002 | 1.004 | 1.003 | 1.004 | - | - | - | 849 | 116 | 578 | 946 | 1588 | - |
| 400 | 1.003 | 1.003 | 1.005 | 1.004 | 1.005 | - | - | - | 1414 | 145 | 1107 | 1426 | 2682 | - |
| 500 | 1.003 | 1.003 | 1.007 | 1.005 | 1.007 | - | - | - | 2189 | 185 | 1729 | 1956 | 4081 | - |
| 600 | 1.004 | 1.004 | 1.008 | 1.009 | 1.008 | - | - | - | 2949 | 206 | 2678 | 2583 | 5843 | - |
| 700 | 1.004 | 1.005 | 1.011 | 1.010 | 1.010 | - | - | - | 3742 | 224 | 4460 | 3949 | 8019 | - |
| 800 | 1.005 | 1.006 | 1.014 | 1.011 | 1.012 | - | - | - | 4878 | 211 | 5635 | 4534 | 11222 | - |
| 900 | 1.006 | 1.006 | 1.016 | 1.013 | 1.014 | - | - | - | 6762 | 271 | 7965 | 5979 | 15460 | - |
| 1000 | 1.006 | 1.007 | 1.022 | 1.016 | 1.016 | - | - | - | 8829 | 287 | 10976 | 7711 | 20087 | - |

### Results: every map

- **Success.** On the 27 other maps' first random scenarios, at four agent counts each, 108
  instances under 30 seconds, `guide` solved 105, LaCAM3 106, EECBS 56 and CBSH2-RTC 5. On
  all 33 maps' first even scenarios, 132 instances, `guide` solved 126 and LaCAM3 125;
  EECBS's and CBSH2-RTC's runs on them were lost to a bug in the script, as above. On
  maze-128-128-1, a maze of corridors one cell wide, `guide` solved only a quarter of the
  agents of its scenarios, and LaCAM3 up to half; on orz900d's even scenario, both solved only
  the first count, 1705 agents.
- **Sum of costs.** On the instances that both solved, `guide`'s total is lower than LaCAM3's
  on 22 of the 26 maps where both solved some of the random scenario, and on 26 of 31 of the
  even ones. Every map where LaCAM3's is lower is one where `guide` fell back on its first
  solution for one instance or more: lak303d, lt_gallowstemplar_n, orz900d and room-64-64-16
  on the random scenarios, and lak303d, orz900d, w_woundedcoast, maze-128-128-10 and
  maze-32-32-4 on the even ones. There, negotiation ran out of time or iterations, and the
  first solution, LaCAM's first search, is returned as it is when the clock stops it, while
  LaCAM3 refines its own for the whole limit. `guide` fell back on 15 of the 108 random
  instances and 18 of the 132 even ones, at the larger counts of the large maps and of the
  narrow mazes.
- **The tracker.** `guide`'s solution is below the tracker's best known sum of costs on 70 of
  the 108 random instances and 62 of the 132 even ones, and its total is within 0.2% to 0.6%
  of the tracker's lower bound on the city maps and 0.4% on den520d. The best known solutions
  on crowded instances are old, as what this does not show, below, has it.
- **Time.** Where negotiation settles, the large maps take seconds: the city maps 1 to 3 s in
  median, the warehouses 0.2 to 2 s, brc202d, w_woundedcoast and the 128 x 128 mazes 10 to
  30 s, as their paths are 500 to 1000 steps long. `guide`'s first solution comes before
  LaCAM3's first on every map but empty-8-8, where both take less than a millisecond; on the city maps,
  where it takes about 260 ms, most of that is working out the distance from every vertex to
  every goal, which LaCAM3 does as it needs them.

Per map, over its four agent counts: instances solved; how many times `guide` fell back on
its first solution; sums of costs over the tracker's best known solutions and over its lower
bounds, summed over the instances both `guide` and LaCAM3 solved (and so dominated by the
largest counts); the instances on which `guide` is below the best known; and median
milliseconds over the instances solved, `guide`'s to its first solution and to the one it
returns, and LaCAM3's to its first.

Random scenarios, first of each map, under 30 seconds:

| Map | Agents | Instances | Solved: guide | LaCAM3 | EECBS | CBSH2-RTC | guide fell back | guide / best | LaCAM3 / best | guide / tracker LB | guide < best | guide first ms | guide ms | LaCAM3 first ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Berlin_1_256 | 250-1000 | 4 | 4 | 4 | 4 | 0 | 0 | 0.981 | 0.986 | 1.003 | 3 | 262 | 1133 | 1348 |
| Boston_0_256 | 250-1000 | 4 | 4 | 4 | 4 | 0 | 0 | 0.975 | 0.982 | 1.006 | 4 | 257 | 1854 | 1351 |
| brc202d | 250-1000 | 4 | 4 | 4 | 3 | 0 | 0 | 0.890 | 0.921 | 1.006 | 4 | 240 | 10532 | 3943 |
| den312d | 250-1000 | 4 | 4 | 4 | 0 | 0 | 1 | 0.731 | 1.038 | 1.221 | 2 | 29 | 16329 | 892 |
| empty-16-16 | 32-128 | 4 | 4 | 4 | 3 | 1 | 0 | 1.034 | 1.172 | 1.156 | 0 | 0 | 6 | 3 |
| empty-48-48 | 250-1000 | 4 | 4 | 4 | 2 | 0 | 0 | 0.878 | 1.116 | 1.121 | 3 | 12 | 629 | 267 |
| empty-8-8 | 8-32 | 4 | 4 | 4 | 4 | 3 | 0 | 1.012 | 1.145 | 1.012 | 0 | 0 | 0 | 0 |
| ht_chantry | 250-1000 | 4 | 4 | 4 | 2 | 0 | 0 | 0.829 | 1.079 | 1.075 | 3 | 41 | 3018 | 1164 |
| ht_mansion_n | 250-1000 | 4 | 4 | 4 | 1 | 0 | 0 | 0.623 | 0.819 | 1.087 | 3 | 49 | 3606 | 1213 |
| lak303d | 250-1000 | 4 | 4 | 4 | 1 | 0 | 1 | 0.960 | 0.878 | 1.348 | 3 | 82 | 8130 | 1726 |
| lt_gallowstemplar_n | 250-1000 | 4 | 4 | 4 | 0 | 0 | 1 | 0.886 | 0.808 | 1.706 | 4 | 56 | 8718 | 1078 |
| maze-128-128-1 | 250-1000 | 4 | 1 | 2 | 0 | 0 | 1 | - | - | - | 0 | 47 | 30160 | 6180 |
| maze-128-128-10 | 250-1000 | 4 | 4 | 4 | 3 | 0 | 0 | 0.866 | 1.035 | 1.045 | 4 | 77 | 11013 | 2572 |
| maze-128-128-2 | 250-1000 | 4 | 4 | 4 | 0 | 0 | 3 | 0.875 | 1.000 | 1.049 | 1 | 145 | 30537 | 1744 |
| maze-32-32-4 | 98-395 | 4 | 4 | 4 | 0 | 0 | 3 | 0.977 | 1.046 | 1.158 | 1 | 10 | 7646 | 188 |
| orz900d | 250-1000 | 4 | 4 | 4 | 1 | 0 | 2 | 0.965 | 0.831 | 1.182 | 3 | 654 | 30572 | 12692 |
| ost003d | 250-1000 | 4 | 4 | 4 | 2 | 0 | 0 | 0.815 | 0.962 | 1.023 | 4 | 68 | 4714 | 1919 |
| Paris_1_256 | 250-1000 | 4 | 4 | 4 | 4 | 0 | 0 | 0.987 | 0.994 | 1.002 | 3 | 255 | 1250 | 1543 |
| random-32-32-10 | 115-461 | 4 | 4 | 4 | 2 | 0 | 0 | 0.933 | 1.220 | 1.253 | 2 | 4 | 210 | 60 |
| random-64-64-10 | 250-1000 | 4 | 4 | 4 | 3 | 0 | 0 | 0.924 | 1.073 | 1.070 | 3 | 25 | 648 | 272 |
| random-64-64-20 | 250-1000 | 4 | 4 | 4 | 2 | 0 | 0 | 0.880 | 1.176 | 1.202 | 3 | 30 | 1941 | 420 |
| room-64-64-16 | 250-1000 | 4 | 4 | 4 | 0 | 0 | 2 | 1.004 | 0.874 | 3.885 | 1 | 35 | 19668 | 820 |
| room-64-64-8 | 250-1000 | 4 | 4 | 4 | 0 | 0 | 1 | 0.828 | 0.958 | 3.318 | 4 | 38 | 18652 | 453 |
| w_woundedcoast | 250-1000 | 4 | 4 | 4 | 3 | 0 | 0 | 0.870 | 0.893 | 1.006 | 4 | 190 | 10791 | 2938 |
| warehouse-10-20-10-2-2 | 250-1000 | 4 | 4 | 4 | 4 | 0 | 0 | 0.938 | 0.967 | 1.013 | 3 | 41 | 806 | 548 |
| warehouse-20-40-10-2-1 | 250-1000 | 4 | 4 | 4 | 4 | 0 | 0 | 0.844 | 0.884 | 1.012 | 3 | 87 | 1706 | 1367 |
| warehouse-20-40-10-2-2 | 250-1000 | 4 | 4 | 4 | 4 | 1 | 0 | 0.982 | 0.984 | 1.001 | 2 | 156 | 990 | 1698 |

Even scenarios, first of each map, under 30 seconds:

| Map | Agents | Instances | Solved: guide | LaCAM3 | EECBS | CBSH2-RTC | guide fell back | guide / best | LaCAM3 / best | guide / tracker LB | guide < best | guide first ms | guide ms | LaCAM3 first ms |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Berlin_1_256 | 237-950 | 4 | 4 | 4 | - | - | 0 | 0.960 | 0.966 | 1.002 | 3 | 250 | 1862 | 2188 |
| Boston_0_256 | 237-950 | 4 | 4 | 4 | - | - | 0 | 0.967 | 0.978 | 1.005 | 3 | 258 | 2579 | 1588 |
| brc202d | 632-2530 | 4 | 4 | 4 | - | - | 3 | 0.889 | 0.904 | 1.005 | 1 | 669 | 31448 | 11011 |
| den312d | 72-290 | 4 | 4 | 4 | - | - | 0 | 0.949 | 1.085 | 1.054 | 3 | 5 | 180 | 50 |
| den520d | 215-860 | 4 | 4 | 4 | - | - | 0 | 0.943 | 0.956 | 1.004 | 4 | 99 | 2156 | 1688 |
| empty-16-16 | 32-128 | 4 | 4 | 4 | - | - | 0 | 1.021 | 1.235 | 1.154 | 0 | 0 | 8 | 4 |
| empty-32-32 | 128-512 | 4 | 4 | 4 | - | - | 0 | 0.939 | 1.178 | 1.183 | 2 | 3 | 157 | 67 |
| empty-48-48 | 288-1152 | 4 | 4 | 4 | - | - | 0 | 0.720 | 0.947 | 1.182 | 3 | 13 | 1223 | 463 |
| empty-8-8 | 8-32 | 4 | 4 | 4 | - | - | 0 | 1.037 | 1.107 | 1.037 | 0 | 0 | 0 | 0 |
| ht_chantry | 115-460 | 4 | 4 | 4 | - | - | 0 | 0.967 | 1.043 | 1.024 | 3 | 19 | 526 | 248 |
| ht_mansion_n | 127-510 | 4 | 4 | 4 | - | - | 0 | 0.926 | 1.038 | 1.035 | 3 | 24 | 904 | 316 |
| lak303d | 260-1040 | 4 | 4 | 4 | - | - | 2 | 0.956 | 0.857 | 1.334 | 3 | 92 | 19304 | 4030 |
| lt_gallowstemplar_n | 155-620 | 4 | 4 | 4 | - | - | 0 | 0.807 | 0.982 | 1.114 | 4 | 33 | 5084 | 663 |
| maze-128-128-1 | 510-2040 | 4 | 1 | 1 | - | - | 1 | - | - | - | 0 | 169 | 30905 | 10961 |
| maze-128-128-10 | 267-1070 | 4 | 4 | 4 | - | - | 2 | 1.246 | 1.086 | 1.563 | 2 | 90 | 23385 | 2181 |
| maze-128-128-2 | 625-2500 | 4 | 4 | 3 | - | - | 4 | - | - | - | 0 | 433 | 33804 | 5362 |
| maze-32-32-2 | 57-230 | 4 | 4 | 4 | - | - | 2 | 0.998 | 1.118 | 1.216 | 1 | 2 | 1081 | 33 |
| maze-32-32-4 | 50-200 | 4 | 4 | 4 | - | - | 1 | 1.263 | 1.195 | 1.462 | 0 | 3 | 424 | 48 |
| orz900d | 1705-6820 | 4 | 1 | 1 | - | - | 1 | 1.001 | 0.889 | 1.273 | 0 | 1962 | 37627 | 27144 |
| ost003d | 202-810 | 4 | 4 | 4 | - | - | 0 | 0.845 | 0.972 | 1.027 | 4 | 55 | 5046 | 1984 |
| Paris_1_256 | 257-1030 | 4 | 4 | 4 | - | - | 0 | 0.948 | 0.967 | 1.002 | 3 | 276 | 2468 | 2550 |
| random-32-32-10 | 22-90 | 4 | 4 | 4 | - | - | 0 | 1.001 | 1.017 | 1.001 | 0 | 1 | 2 | 3 |
| random-32-32-20 | 25-100 | 4 | 4 | 4 | - | - | 0 | 1.005 | 1.044 | 1.024 | 0 | 1 | 4 | 4 |
| random-64-64-10 | 50-200 | 4 | 4 | 4 | - | - | 0 | 1.001 | 1.006 | 1.001 | 0 | 5 | 28 | 28 |
| random-64-64-20 | 55-220 | 4 | 4 | 4 | - | - | 0 | 1.001 | 1.015 | 1.007 | 0 | 6 | 37 | 25 |
| room-32-32-4 | 32-130 | 4 | 4 | 4 | - | - | 0 | 1.012 | 1.191 | 1.188 | 2 | 1 | 30 | 10 |
| room-64-64-16 | 100-400 | 4 | 4 | 4 | - | - | 0 | 0.808 | 1.046 | 1.324 | 3 | 12 | 3291 | 288 |
| room-64-64-8 | 77-310 | 4 | 4 | 4 | - | - | 0 | 0.949 | 1.213 | 1.233 | 3 | 8 | 880 | 100 |
| w_woundedcoast | 492-1970 | 4 | 4 | 4 | - | - | 2 | 0.964 | 0.813 | 1.379 | 2 | 394 | 30371 | 11872 |
| warehouse-10-20-10-2-1 | 112-450 | 4 | 4 | 4 | - | - | 0 | 0.988 | 1.069 | 1.032 | 3 | 13 | 379 | 194 |
| warehouse-10-20-10-2-2 | 122-490 | 4 | 4 | 4 | - | - | 0 | 0.999 | 1.006 | 1.003 | 1 | 18 | 195 | 248 |
| warehouse-20-40-10-2-1 | 230-920 | 4 | 4 | 4 | - | - | 0 | 0.898 | 0.920 | 1.006 | 3 | 75 | 1514 | 1434 |
| warehouse-20-40-10-2-2 | 250-1000 | 4 | 4 | 4 | - | - | 0 | 0.955 | 0.957 | 1.001 | 3 | 147 | 1091 | 1464 |

### What it took: safe intervals and a first solution

Each step was measured on two random scenarios of 13 maps, at two of each map's larger agent
counts, or one on ost003d and brc202d (26 instances), with a limit of 30 seconds, on a machine
shared with up to 150 other jobs, so that times are rough. Sums of costs are over the lower
bound, on the instances that every column solved.

| Map, instances | Through time (before) | Safe intervals, history where the overuse is | History from a goal's holder on, always | ... once the present factor is at its most (default) |
| --- | --- | --- | --- | --- |
| empty-32-32, 4 | 4, 1.186 | 3, 1.171 | 4, 1.187 | **4, 1.171** |
| random-32-32-20, 4 | 4, 1.230 | 4, 1.211 | 4, 1.224 | **4, 1.211** |
| room-32-32-4, 4 | 4, 1.214 | 4, 1.209 | 4, 1.210 | **4, 1.209** |
| maze-32-32-2, 4 | 4, 1.245 | 4, 1.215 | 4, 1.251 | **4, 1.215** |
| warehouse-10-20-10-2-1, 4 | 4, 1.068 | 4, 1.059 | 4, 1.089 | **4, 1.059** |
| den520d, 4 | 4, 1.0068 | 4, 1.0059 | 4, 1.0072 | **4, 1.0059** |
| maze-32-32-4, 4 | 2, 1.179 | 2, 1.160 | 2, 1.160 | **2, 1.158** |
| random-64-64-20, 4 | 3, 1.194 | 4, 1.175 | 4, 1.208 | **4, 1.175** |
| room-64-64-8, 4 | 2, 1.297 | 4, 1.273 | 4, 1.265 | **4, 1.273** |
| lak303d, 4 | 2, 1.020 | 2, 1.017 | 2, 1.022 | **2, 1.017** |
| maze-128-128-2, 4 | 0 | 1 | 2 | **2** |
| ost003d, 2 | 0 | 2 | 0 | **2** |
| brc202d, 2 | 0 | 1 | 0 | **1** |
| Expansions, all 26 | 775 M | 305 M | 335 M | **300 M** |

"Solved" is by negotiation alone: these runs came before the first solution to fall back on.

- **Safe intervals** (`Method::SafeIntervals`, the default; the [module
  documentation](../src/mapf/mod.rs) has how). Searches over a vertex and an interval of time
  over which nothing there changes, rather than over a vertex at each time, with conflicts
  priced, as negotiation needs, and the engine's schedule run over a table of the plans kept
  per vertex in time order. Every map's sum of costs went down, by up to 3 points of the lower
  bound, and 26 instances took 305 million expansions where the time-expanded space took 775
  million, and failed on more of the large maps. On maze-128-128-2, a maze with corridors two
  cells wide and paths of 1500 steps, the time-expanded space solved none within the limit:
  with 100 agents, it ran 33 iterations in a minute at 98 million expansions. Searching over
  safe intervals with intervals kept up to date in the table, rather than worked out by each
  search, and looking at edges for swaps only where an interval ends, took a further 20% to
  35% off the time, as profiling had each at a quarter of it.
- **History from a goal's holder on.** Negotiation over safe intervals left two agents in a
  dead end in conflict for good, one on its goal at the mouth and one that has to pass it to
  get to its own: history on the time they collided sent the second a step later each
  iteration, never far enough. History on every time from the holder's arrival makes arriving
  early and passing late both dear. Always on, it made sums of costs higher on most maps, as
  agents arrived on their goals later than they needed to; on only once the present factor is
  at its most, which few instances reach, it solves the dead ends and changes little else.
- **A first solution, by LaCAM** (`SolveOptions::first`, on by default;
  `mapf::first_solution`). LaCAM's first search over PIBT's steps, with LaCAM3's swap for
  corridors: without it, PIBT alone and LaCAM over it both went round for good on warehouse
  with 300 agents. It is complete, but a search over a thousand agents' configurations can grow
  without end, so it stops at 10 million agent positions and 10 million constraints, about
  400 MB; without the second, maze-128-128-1 with 1000 agents grew to tens of gigabytes. It
  takes milliseconds where negotiation takes seconds, at sums of costs 37% higher, and is what
  `solve` returns when negotiation doesn't settle, shortened unless the clock stopped it:
  maze-32-32-4 with half its agents and more, for one, where agents have to wait for each other
  in the order of their goals along a corridor, and negotiation's best responses go round.
- **Tried and dropped:**
  - **Prioritized planning as the first iteration**: each agent planned over hard safe
    intervals against those before, within ten times its shortest path's expansions, and
    priced as before when that fails. Sums of costs up to 5 points higher on the small maps
    and the same elsewhere, and no faster: as with a first present factor of 1000 through
    time, the agents planned first take what the others need.
  - **A higher first present factor**, 2 or 8 rather than 0.5: sums of costs up to 2 points
    higher, and more expansions on the large maps.
  - **Weighted A\***, bounds times 1.2 or 1.5: 1.3 to 1.8 times faster, for sums of costs up
    to 2 and 6 points higher, and fewer solved.
  - **Starting negotiation again** from the present factor of the first iteration, history
    kept, after 10 or 20 iterations without fewer conflicts, as `asic` does, with up to 400
    iterations: it solved some of the maze-32-32-4 instances that negotiation alone can't, in
    10 to 60 times the time, and not others, which the first solution covers instead.
- **Nothing in the engine changed.** The negotiation over safe intervals is the engine's
  loop, in `src/mapf`, because the engine's table of resources can't say when a vertex is next
  free; the time-expanded space still goes through the engine as before.

#### Through time: the first results

Each step was measured on the first 5 scenarios of each map, with the same agent counts and a
limit of 10 seconds, so 50 instances a map. The sum of costs over the lower bound is on the
instances that every column solved, so the columns compare; "solved" counts all 50. Runs
shared the machine with up to 150 others, and times on it are too noisy to quote, but for the
first step, which was timed alone.

| Map | First version | Chains of stays | History 0.5 | Shortening (default) | First present 1000 | Growth 1.3 | 200 neighbourhoods, no shortening |
| --- | --- | --- | --- | --- | --- | --- | --- |
| empty-32-32 | 1.225 | 1.175 | 1.164 | **1.153** | 1.346 | 1.160 | 1.155 |
| random-32-32-20 | 1.240 | 1.206 | 1.195 | **1.178** | 1.313 | 1.199 | 1.176 |
| room-32-32-4 | 1.222 | 1.213 | 1.202 | **1.179** | 1.270 | 1.206 | 1.175 |
| maze-32-32-2 | 1.232 | 1.198 | 1.192 | **1.182** | 1.250 | 1.196 | 1.179 |
| warehouse, solved | 1.057, 50 | 1.036, 46 | 1.035, 47 | **1.033, 48** | 1.211, 42 | 1.035, 40 | 1.032, 49 |
| den520d, solved | 1.0022, 34 | 1.0019, 43 | 1.0018, 49 | **1.0017, 48** | 1.0438, 11 | 1.0018, 43 | 1.0016, 50 |

- **Chains of stays.** The first version ended each route with one step from the goal into its
  end, which held the goal at every time to the horizon. A search priced that step over all
  of those times each time it reached the goal, at each time it did, and on den520d, with a
  horizon of about 420, that was most of the time. A stay per time, shared by every arrival,
  took 600 agents there from 10.4 s to 4.7 s and solved more of den520d within the limit.
  The step costs had to change for it: moves and waits cost 2 and stays 1, so that a route
  still costs its arrival time plus a constant, and every step has a base cost for the price
  of overuse to multiply. It also gave lower sums of costs everywhere, as the engine's pricing
  of a goal now rises a time at a time. On the small maps it is slower, as a search walks the
  stays to the horizon: 400 agents on empty-32-32 went from 0.43 s to 0.60 s, run alone.
- **History 0.5** (`mapf::engine_options`). A move costs 2, so the engine's default history
  of 1 is half a move per unit of overuse; a quarter of one gave sums of costs up to 1%
  lower and solved as many or more. An eighth was no better.
- **Shortening** (`SolveOptions::improve_passes`, 10 by default): after a legal solution,
  each agent is planned again by an exact search against the others as hard reservations,
  and keeps what arrives sooner. It took the ratio down by up to 2.3 points, least on the
  large maps, where it was already close to 1, for 15% to 30% more time.
- **Tried and dropped:**
  - **A first iteration close to prioritized planning**, with a first present factor of 1000
    so that each agent keeps off what earlier ones took, as MAPF-LNS2's initial solution does
    with soft conflicts. It was worse on every map, and solved 11 of 50 on den520d: the agents
    planned first take what the others need, and history has to undo it.
  - **A present factor growth of 1.3**, VPR's: ratios 0.2 to 1.6 points lower than with 1.8
    at the time, for 1.2 to 1.7 times the time, and fewer solved on warehouse.
  - **Neighbourhoods**: a late agent and the agents in the way of its shortest path, routed
    again by negotiation with everyone else's resources at a capacity of 0, and kept if they
    arrived sooner in total, as MAPF-LNS does with prioritized planning. 200 neighbourhoods of
    up to 8 agents did about as well as shortening alone, as the table shows, and after
    shortening gained up to 1% more, for 1.5 to 22 times the time. 1000 did no better than 200.
  - **Incremental rerouting** changed nothing, as each agent is one connection, and **a slack
    of 4** for the first horizon rather than 16 was faster by a few percent and slightly
    worse.
- **Nothing in the engine changed.** Every one of these is in `src/mapf`, through the
  `RoutingSpace` it implements and the options the engine already had.

### What this does not show

- **Few instances on most maps.** The second set has one random and one even scenario a map,
  at four agent counts, under 30 seconds, because it had to share the machine with two solver
  processes at most; the 25 scenarios a map of the first set, and the tracker's every count
  from 1 up, would say more. EECBS and CBSH2-RTC have no results on the even scenarios.
- **The tracker's best known solutions** are far from the best on many crowded instances: on
  den520d with 1000 agents, for one, the best known is 184 788, from December 2022, where
  `guide` found 169 152 and the lower bound is 167 907. That `guide` is below the best known on
  so many instances says how few solvers that refine for long have been run on them since,
  more than how good it is: LaCAM3's total, given its 30 seconds, is below the best known on
  about half the maps too. The plans were not submitted.
- **One time limit, one thread, one seed.** Results at other time limits differ: LaCAM3 keeps
  improving with time, where `guide` stops at its first legal solution, shortened, and one that
  is stopped by the clock returns its first solution, LaCAM's, unshortened.
- **Sum of costs only.** `guide` reports the makespan but does not minimize it.
- **Other variants** of the problem, such as agents that disappear at their goals, k-robust
  plans, or lifelong MAPF, are not modelled.

## ASIC global routing against the ISPD contests

M10's ASIC item: global routing of the ISPD 2008 contest's 16 designs, and of the ISPD 2007
contest's 2D and 3D ones, against the contests' published results and the routers that came
after them, by the contests' own measures: total overflow (TOF), maximum overflow (MOF), and
wirelength (WL), which counts each edge between two cells that wires cross and, for each layer
a via spans, 1 in 2008 and 3 in 2007. Overflow ranks first: a router with less total overflow
wins whatever its wirelength. Measured on 2026-11-17 with rustc 1.97.1, on the commit that
added this round's results. The second round's, on `732bf59`, and the first results, on
`86023ce`, are kept below for comparison.

### How

```text
scripts/ispd/fetch.sh DIR                       # the benchmarks and the contests' scripts
scripts/ispd/run.sh DIR WORK                    # route and score the 16 ISPD 2008 designs
python3 scripts/ispd/tables.py WORK             # the table below
SUITE=ispd07-2d scripts/ispd/run.sh DIR WORK07  # and the 2007 ones
```

- **The designs** are downloaded from the contests' pages at ispd.cc by
  [`scripts/ispd/fetch.sh`](../scripts/ispd/fetch.sh), which also fetches `eval2008.pl` and the
  2007 contest's scripts. The pages post them for research use and state no licence, so they
  are not copied here. The 2007 contest had adaptec1 to 5 and newblue1 to 3, each as a 2D
  problem of two layers and a 3D one of six, and 2008 reused the eight 3D ones and added bigblue1
  to 4 and newblue4 to 7. Every net is of width 1 on layers of width 1 and spacing 1, so each
  net takes 2 of an edge's capacity, and every pin is on layer 1.
- **The router** is [`asic::route_design`](../src/asic/router.rs) with its defaults, through
  [`examples/ispd_route.rs`](../examples/ispd_route.rs), which writes the contest's route format.
  It routes all nets of at most 1000 pins whose pins are in more than one cell; the contests
  require no others.
- **Scoring** is [`asic::evaluate`](../src/asic/eval.rs), which follows `eval2008.pl` rule by
  rule: usage of the net's width or the layer's, whichever is more, plus spacing, for each edge
  each wire crosses; overflow in those units, so that one net too many on an edge is an overflow
  of 2; and the script's checks that every segment is connected to the net's first pin and every
  pin is reached. `eval2008.pl` (perl 5.44) printed the same total overflow, maximum overflow
  and wirelength, to the unit, as the evaluator for every file it was given: the handmade
  cases of [`src/asic/tests.rs`](../src/asic/tests.rs), faults included, the route files of
  the first results' 14 ISPD 2008 designs, the second round's of adaptec4, bigblue2, newblue1
  and newblue5, and this round's of newblue3. The 2007 contest's script, `grc_eval`, counts 3 for each via
  and is scored here as `eval2008.pl` with that via cost; its other rules were not reproduced.
- **Published results** are from the papers' tables as printed, with their machines: the 2008
  contest's final results (Sze, Nam and Yildiz, ISPD 2008 slides, `S7-3.pdf`): wall time on a
  2.8 GHz Opteron 8220, at most 4 threads; NTHU-Route 2.0 (Chang, Lee and Wang, ICCAD 2008,
  Tables III and IV), on a 2.2 GHz Opteron; NCTU-GR 2.0 (Liu, Kao, Li and Chao, IEEE TCAD 32(5),
  2013, Tables V and VI), whose single-threaded router SGR and FastRoute 4.1 were run on the same
  3.0 GHz Xeon. NTHU-Route, an earlier version of NTHU-Route 2.0, won the 2008 contest.
- **Runtime** here is wall time and CPU time of the whole run, reading and scoring included, on
  an AMD Ryzen Threadripper PRO 9995WX shared with other work: the twelve designs that converge
  on one thread each, for a comparison with SGR's single thread, and the four that do not on 3
  threads each, with at most 16 threads of this work at once, under `nice`, beside other jobs
  that kept the machine's load between 15 and 180. Times are rough for that; the expansions,
  which no machine or thread count changes, are the measure of work, and results do not depend
  on the thread count. Neither is comparable with the published times, which are of other
  machines from 2008 to 2013.

The flow is the one most of the contests' routers share, built on `guide`'s engine:

1. **The plane.** Each layer's capacity becomes tracks, its capacity over a wire's width and
   spacing, and the tracks of all layers are added up onto one plane, an `asic::PlanarSpace`.
   Its nodes are cells twice over, for arriving along a row and along a column, so that a turn
   can cost 1 more than a straight step: a turn on the plane is a via once layers are given.
2. **Pattern routes.** Each net's pins are joined by a rectilinear Steiner tree, by the
   iterated 1-Steiner heuristic for nets of up to 64 pins and a minimum spanning tree above,
   and each of its edges is laid as whichever L shape, or Z shape with two turns, costs less
   against the nets before, with the net's own edges free, so that its shapes overlap where
   they can; nets go shortest first. These are the engine's seeds (`Engine::seed_routes`), so
   that it only routes again what overflows.
3. **Negotiation.** The engine routes again the nets on edges that overflow, longest first, by
   A\* within a box around each net's pins, 10 cells larger, that grows by 5 each iteration the
   net ends on an edge that overflows, up to 40, in batches of 32 on as many threads as given.
   A first try has `EngineOptions::crowding` at 1, to the 8th power
   (`EngineOptions::crowding_exponent`), which settles most designs in a few iterations. If it
   goes 3 iterations without halving the overflow, while that is above a thousandth of its
   first iteration's, negotiation starts from the pattern routes again, with no history and the
   boxes as they were, at crowding 16: a step into an edge costs 1.06 times as much at half
   full, 2.6 times at three quarters, 7.9 times at 90% and 17 times when full, so that nets
   keep out of what is nearly full while there is room elsewhere. When 15 iterations go by
   without 1% less overflow, negotiation starts again from its best routes so far, with the
   history it has learned but the present factor back at its start, up to 20 times, until two
   passes in a row end without a new least or 500 iterations have been run in all, not
   counting the first try's.
4. **Overflow that is left**, on a design that negotiation leaves with some, is routed again,
   the nets on it each priced at the overflow it would add (`EngineOptions::marginal_overuse`)
   rather than at how far over each edge would be, at a present factor of 1000 and without
   history, for as long as each iteration brings the total down, up to 10.
5. **Refinement.** Once the plane is free of overflow, the nets longer than a Steiner tree of
   their pins, which went round something on the way, are routed again once each, by their
   shortest way that overflows nothing, against the rest as they are, with crowding at 1.
6. **Layers.** Each net's tree is given layers exactly for the fewest vias that fit, by dynamic
   programming over the tree as in COLA (Lee and Wang, 2008), with an edge on a layer costing a
   twentieth of a via more the fuller it would be, so that of two layers that take as many
   vias the net goes on the emptier, nets with the least wire per pin first; nets on an edge
   that overflows on its layer while another layer of it has room are given layers again with
   that edge dearer, up to 4 times.

### Results: ISPD 2008

Each design routed once with the defaults, scored by `asic::evaluate`. Wire is the length of
wires alone and vias the layers they span, so that WL is their sum. Iterations count every pass
of negotiation, the first try's included, then those that take overflow down and the
refinement's one, and restarts the passes after the first at the steeper crowding. The twelve
that converge ran on one thread, and the four that do not on 3:

| Benchmark | TOF | MOF | WL | Wire | Vias | Iterations | Restarts | Wall s | CPU s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| adaptec1 | 0 | 0 | 5 362 676 | 3 582 440 | 1 780 236 | 10 | 0 | 238 | 236 |
| adaptec2 | 0 | 0 | 5 227 458 | 3 306 907 | 1 920 551 | 10 | 0 | 37 | 36 |
| adaptec3 | 0 | 0 | 13 036 717 | 9 569 598 | 3 467 119 | 8 | 0 | 214 | 213 |
| adaptec4 | 0 | 0 | 12 132 094 | 8 935 612 | 3 196 482 | 6 | 0 | 37 | 36 |
| adaptec5 | 0 | 0 | 15 655 403 | 10 373 874 | 5 281 529 | 30 | 0 | 621 | 617 |
| bigblue1 | 0 | 0 | 5 714 306 | 3 781 138 | 1 933 168 | 13 | 0 | 426 | 423 |
| bigblue2 | 0 | 0 | 9 106 200 | 4 888 050 | 4 218 150 | 165 | 2 | 1122 | 1115 |
| bigblue3 | 0 | 0 | 13 177 650 | 7 909 258 | 5 268 392 | 25 | 0 | 492 | 489 |
| bigblue4 | 320 | 4 | 23 839 666 | 12 878 834 | 10 960 832 | 265 | 7 | 5350 | 14952 |
| newblue1 | 0 | 0 | 4 666 642 | 2 433 284 | 2 233 358 | 374 | 4 | 3143 | 3129 |
| newblue2 | 0 | 0 | 7 553 192 | 4 630 351 | 2 922 841 | 5 | 0 | 16 | 15 |
| newblue3 | 31832 | 1222 | 10 726 120 | 7 723 248 | 3 002 872 | 175 | 9 | 7580 | 18682 |
| newblue4 | 180 | 2 | 13 338 251 | 8 660 049 | 4 678 202 | 400 | 7 | 3070 | 8392 |
| newblue5 | 0 | 0 | 23 326 746 | 14 836 188 | 8 490 558 | 28 | 0 | 1261 | 1254 |
| newblue6 | 0 | 0 | 17 836 777 | 10 276 303 | 7 560 474 | 12 | 0 | 270 | 268 |
| newblue7 | 166 | 4 | 35 541 372 | 19 240 192 | 16 301 180 | 508 | 4 | 10526 | 28101 |

Against the published results, the contest's winner, NCTU-GR 2.0's single-threaded router SGR
and FastRoute 4.1 as NCTU-GR 2.0 ran it. "Best of contest" is the least total overflow of any
entrant, and wirelength is only compared where both are free of overflow; seconds are wall
time here and in the contest, and SGR's minutes:

| Benchmark | TOF: guide | NTHU-Route (contest) | Best of contest | SGR | FastRoute 4.1 | WL / NTHU-Route | WL / SGR | WL / FastRoute 4.1 | Seconds: guide, NTHU-Route, SGR |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| adaptec1 | 0 | 0 | 0 | 0 | 0 | 1.002 | 1.011 | 0.998 | 238, 475, 151 |
| adaptec2 | 0 | 0 | 0 | 0 | 0 | 0.999 | 1.015 | 1.002 | 37, 104, 39 |
| adaptec3 | 0 | 0 | 0 | 0 | 0 | 0.994 | 1.009 | 0.997 | 214, 481, 204 |
| adaptec4 | 0 | 0 | 0 | 0 | 0 | 0.997 | 1.007 | 1.001 | 37, 135, 74 |
| adaptec5 | 0 | 0 | 0 | 0 | 0 | 1.006 | 1.017 | 1.005 | 621, 1030, 362 |
| bigblue1 | 0 | 0 | 0 | 0 | 0 | 1.015 | 1.015 | 1.009 | 426, 586, 245 |
| bigblue2 | 0 | 0 | 0 | 0 | 0 | 1.005 | 1.027 | 0.999 | 1122, 594, 253 |
| bigblue3 | 0 | 0 | 0 | 0 | 0 | 1.008 | 1.019 | 1.013 | 492, 259, 115 |
| bigblue4 | 320 | 182 | 182 | 194 | 130 |  |  |  | 5350, 7533, 3614 |
| newblue1 | 0 | 0 | 0 | 0 | 0 | 1.003 | 1.020 | 1.007 | 3143, 306, 142 |
| newblue2 | 0 | 0 | 0 | 0 | 0 | 0.998 | 1.014 | 1.005 | 16, 67, 38 |
| newblue3 | 31832 | 31454 | 31106 | 31710 | 31276 |  |  |  | 7580, 7746, 8600 |
| newblue4 | 180 | 152 | 142 | 144 | 136 |  |  |  | 3070, 4023, 1040 |
| newblue5 | 0 | 0 | 0 | 0 | 0 | 1.007 | 1.020 | 1.010 | 1261, 854, 316 |
| newblue6 | 0 | 0 | 0 | 0 | 0 | 1.008 | 1.016 | 1.003 | 270, 818, 266 |
| newblue7 | 166 | 68 | 68 | 58 | 54 |  |  |  | 10526, 8433, 5140 |

- **Overflow.** All 12 of the designs that NTHU-Route, SGR and FastRoute 4.1 route free of
  overflow are free of it here. newblue3 is left with 31 832, from 65 046, confirmed by
  `eval2008.pl`: 1.2% more than NTHU-Route's 31 454 and 2.3% more than the contest's best,
  31 106, where it was twice theirs. Its 3D overflow is now its plane's, twice its 15 920
  tracks over, and the 2D design scores the same. bigblue4, newblue4 and newblue7 are left as
  they were, with 320, 180 and 166, within the contest's range of 182 to 472 on bigblue4 and of
  142 to 262 on newblue4, and behind its best of 68 on newblue7.
- **Wirelength**, where both are free of overflow, is 0.35% above the contest's winner over the
  twelve, by geometric mean, 1.6% above SGR and 0.4% above FastRoute 4.1, from 0.6% below to
  2.7% above each, where the second round was 0.9%, 2.2% and 1.0% above. It is below
  NTHU-Route's on adaptec2, adaptec3, adaptec4 and newblue2. The eight that the first try
  settles have 0.1% to 1.4% less than in the second round; the four that go on at the steeper
  crowding have the same routes as before.
- **Runtime.** On one thread, the twelve took 15 s to 52 minutes of CPU time, where SGR took 38
  to 362 s on one thread of a 3.0 GHz Xeon from 2012: from 0.39 to 4.3 times its time on the
  ten that converge without restarts, where the second round took 0.7 to 8.6 times on 12
  threads' CPU, and 4.4 and 22 times on bigblue2 and newblue1, which take 165 and 374
  iterations. Run alongside on one thread each, the second round's defaults took 1.2 to 4.4
  times as long as this round's on the eight that the first try settles, and this round's took
  0.9 to 1.2 times as long on the four that it does not, whose first try is spent for nothing.

Before and after, the second round against this one, with expansions, which do not depend on
the machine, and CPU seconds on one thread, run alongside each other:

| Benchmark | TOF before | TOF now | WL before | WL now | WL change | Expansions before | Expansions now | CPU s before | CPU s now |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| adaptec1 | 0 | 0 | 5 435 326 | 5 362 676 | -1.3% | 3.73e9 | 1.52e9 | 593 | 236 |
| adaptec2 | 0 | 0 | 5 284 412 | 5 227 458 | -1.1% | 0.97e9 | 0.19e9 | 159 | 36 |
| adaptec3 | 0 | 0 | 13 138 240 | 13 036 717 | -0.8% | 2.24e9 | 1.14e9 | 431 | 213 |
| adaptec4 | 0 | 0 | 12 144 323 | 12 132 094 | -0.1% | 0.22e9 | 0.08e9 | 65 | 36 |
| adaptec5 | 0 | 0 | 15 786 256 | 15 655 403 | -0.8% | 7.08e9 | 3.91e9 | 1235 | 617 |
| bigblue1 | 0 | 0 | 5 796 678 | 5 714 306 | -1.4% | 6.67e9 | 2.71e9 | 1133 | 423 |
| bigblue2 | 0 | 0 | 9 106 200 | 9 106 200 | 0.0% | 6.42e9 | 7.10e9 | 1209 | 1115 |
| bigblue3 | 0 | 0 | 13 177 650 | 13 177 650 | 0.0% | 2.37e9 | 3.26e9 | 401 | 489 |
| bigblue4 | 320 | 320 | 23 839 646 | 23 839 666 | +0.0% | 88.1e9 | 90.0e9 | | |
| newblue1 | 0 | 0 | 4 666 642 | 4 666 642 | 0.0% | 22.97e9 | 23.29e9 | 3480 | 3129 |
| newblue2 | 0 | 0 | 7 565 097 | 7 553 192 | -0.2% | 0.04e9 | 0.02e9 | 18 | 15 |
| newblue3 | 65 046 | 31 832 | 10 873 493 | 10 726 120 | -1.4% | 97.2e9 | 101.4e9 | | |
| newblue4 | 180 | 180 | 13 338 476 | 13 338 251 | -0.0% | 47.4e9 | 48.7e9 | | |
| newblue5 | 0 | 0 | 23 326 746 | 23 326 746 | 0.0% | 5.79e9 | 8.10e9 | 1028 | 1254 |
| newblue6 | 0 | 0 | 18 060 450 | 17 836 777 | -1.2% | 4.42e9 | 1.60e9 | 726 | 268 |
| newblue7 | 166 | 166 | 35 540 959 | 35 541 372 | +0.0% | 182.8e9 | 184.8e9 | | |

### Results: ISPD 2007

The 2007 contest's 3D designs are 2008's first eight, and routing does not depend on how vias
are scored, so the 3D results are the 2008 runs scored with 3 for each via. The 2D designs
are the same nets on two layers, horizontal on layer 1 and vertical on layer 2, with the same
capacities added up; they route the same on the plane, and differ in their vias. Against
NTHU-Route 2.0's tables, the best published on these:

2D, two layers, as routed, on 4 threads, or 2 for newblue1 and 3 for newblue3:

| Benchmark | TOF | MOF | WL | Wire | Vias | Iterations | Wall s | CPU s | NTHU-Route 2.0: TOF, WL, min | WL / NTHU-Route 2.0 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| adaptec1 | 0 | 0 | 5 675 936 | 3 582 440 | 697 832 | 10 | 119 | 400 | 0, 55.91e5, 9.7 | 1.015 |
| adaptec2 | 0 | 0 | 5 449 510 | 3 306 907 | 714 201 | 10 | 30 | 67 | 0, 54.01e5, 1.9 | 1.009 |
| adaptec3 | 0 | 0 | 13 454 163 | 9 569 598 | 1 294 855 | 8 | 125 | 361 | 0, 134.39e5, 10.3 | 1.001 |
| adaptec4 | 0 | 0 | 12 860 551 | 8 935 612 | 1 308 313 | 6 | 41 | 56 | 0, 127.81e5, 2.2 | 1.006 |
| adaptec5 | 0 | 0 | 16 037 484 | 10 373 874 | 1 887 870 | 30 | 336 | 1084 | 0, 157.03e5, 20.5 | 1.021 |
| newblue1 | 0 | 0 | 4 894 760 | 2 433 284 | 820 492 | 374 | 3518 | 3499 | 0, 48.60e5, 5.5 | 1.007 |
| newblue2 | 0 | 0 | 7 955 755 | 4 630 351 | 1 108 468 | 5 | 20 | 23 | 0, 78.54e5, 1.1 | 1.013 |
| newblue3 | 31832 | 1222 | 11 311 290 | 7 723 248 | 1 196 014 | 175 | 7560 | 18618 | 31454, 110.82e5, 120.9 |  |

3D, six layers: the 2008 routes, scored with 3 for each via, so that iterations and times are
in the 2008 table:

| Benchmark | TOF | MOF | WL | Wire | Vias | Iterations | Wall s | CPU s | NTHU-Route 2.0: TOF, WL, min | WL / NTHU-Route 2.0 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| adaptec1 | 0 | 0 | 8 923 148 | 3 582 440 | 1 780 236 |  |  |  | 0, 88.81e5, 10.0 | 1.005 |
| adaptec2 | 0 | 0 | 9 068 560 | 3 306 907 | 1 920 551 |  |  |  | 0, 90.88e5, 2.1 | 0.998 |
| adaptec3 | 0 | 0 | 19 970 955 | 9 569 598 | 3 467 119 |  |  |  | 0, 200.84e5, 10.9 | 0.994 |
| adaptec4 | 0 | 0 | 18 525 058 | 8 935 612 | 3 196 482 |  |  |  | 0, 185.99e5, 2.6 | 0.996 |
| adaptec5 | 0 | 0 | 26 218 461 | 10 373 874 | 5 281 529 |  |  |  | 0, 260.18e5, 23.0 | 1.008 |
| newblue1 | 0 | 0 | 9 133 358 | 2 433 284 | 2 233 358 |  |  |  | 0, 90.96e5, 6.2 | 1.004 |
| newblue2 | 0 | 0 | 13 398 874 | 4 630 351 | 2 922 841 |  |  |  | 0, 134.63e5, 1.2 | 0.995 |
| newblue3 | 31832 | 1222 | 16 731 864 | 7 723 248 | 3 002 872 |  |  |  | 31454, 166.95e5, 122.4 |  |

newblue3, which no router solves, is left with 31 832 in both, from 37 428 in 2D and 65 046 in
3D, 1.2% more than NTHU-Route 2.0's 31 454: the layers now add nothing to the plane's
overflow, and the plane has 15% less. Wirelength is that of NTHU-Route 2.0 in 3D by geometric
mean over the seven others, from 0.6% below to 0.8% above, where vias count three times, and
1.0% above it in 2D, from 0.1% to 2.1% above, where the second round was 0.6% and 1.9% above.
With two layers, a net has no choice of layer, so that in 2D the gain is nearly all the wire's.

### What it took

#### The first results

On adaptec2, from the first version to the first results' defaults, each step in turn, on 24
to 48 threads of a machine shared with the other runs, so that the times are rough:

| Version | TOF | WL | Wire | Vias | Iterations | Expansions | Route s |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Maze routing alone, batches of 64, no boxes | 4 | 6 119 165 | 3 567 851 | 2 551 314 | 100 | 5.74e9 | 177 |
| Boxes 10 cells beyond the pins, batches of 16 | 0 | 6 147 473 | 3 583 444 | 2 564 029 | 72 | 1.77e9 | 146 |
| A turn costs 1 | 0 | 5 729 581 | 3 505 618 | 2 223 963 | 59 | 1.02e9 | 83 |
| Pattern routes as seeds | 0 | 5 487 121 | 3 346 520 | 2 140 601 | 47 | 0.64e9 | 53 |
| Layers given in order of wire per pin, not of length | 0 | 5 374 857 | 3 346 520 | 2 028 337 | 47 | 0.64e9 | 59 |
| `crowding` 1, batches of 32: the first results' defaults | 0 | 5 375 045 | 3 372 221 | 2 002 824 | 10 | 0.38e9 | 25 |

- **Turns.** Maze routes take any of the many equally short paths, and a staircase has a turn,
  and so a via, at every step. Pricing a turn at 1 halved the cells where wires turn, from
  757 000 to 442 000, and took 13% off the vias. 2 and 3 instead of 1 changed the wirelength by
  less than 0.2% either way, on adaptec2 and newblue2.
- **Pattern routes** gave 4.5% less wire than maze routing from nothing, and fewer turns again.
- **Layer order** matters: nets in order of length, shortest first, gave 2.14 million vias;
  longest first 2.80 million; by congestion, most congested first, 2.42 million; by pins, fewest
  first, 2.31 million; by wire per pin, least first, 2.03 million. Assigning every net again
  against all the others afterwards changed nothing, as no net can do better than its exact
  assignment against the rest. Negotiating layers instead, letting low layers overflow at first
  at a rising price, was far worse on newblue2: 4.33 million vias against 3.03 million.
- **Crowding.** Plain negotiation only prices an edge once it overflows, and on these grids,
  with 30 to 40 tracks to an edge, the first routes overflow thousands of edges by a little, and
  the last few take tens of iterations. With `crowding` at 1, adaptec2 converged in 10 iterations
  rather than 51, for the same wirelength: 0.7% more wire and 1.3% fewer vias. At 0.5 it took 29
  iterations, for 0.2% less, and at 2 and 4 it took 10, for 0.7% and 1.9% more.
- **Batches and threads** change results, as on FPGAs, and late in a run nets that fight over
  the same edge move all at once in a batch. Before crowding, batches of 64 left an edge
  overflowing on adaptec2 after 100 iterations where 32 converged in 51.
- **Weighted A\*** (`EngineOptions::bound_weight` 1.2, 1.5, 2) took 0.9, 1.06 and 1.9 times the
  expansions on adaptec1: with the node re-expansions that an inconsistent bound brings, it does
  not pay here.

#### Convergence on the three that stalled

From the first results to the defaults, on bigblue2, newblue1 and newblue5, each step against
the one before. Counts are edges overflowing on the plane after 40 iterations, and expansions
those of the fortieth, on 12 threads each with many runs sharing the machine:

| Version | bigblue2 | newblue1 | newblue5 | Expansions, newblue1 |
| --- | --- | --- | --- | --- |
| The first results' defaults | 94 | 322 | 18 | 708e6 |
| Steiner pattern routes, layers on the emptier layer | 123 | 295 | 15 | 692e6 |
| `crowding` 2.5 to the 8th power | 71 | 218 |  | 630e6 |
| `crowding` 6 to the 8th power | 54 | 198 |  | 619e6 |
| `crowding` 10 to the 8th, boxes grown by 40 cells at most | 53 | 154 | 0 by the 12th | 117e6 |
| `crowding` 16 to the 8th, nets longest first | 36 | 131 |  | 112e6 |
| Started again when stalled, 15 iterations without 1% less: the defaults | 27 | 137 | 0 by the 18th | 117e6 |

The last row's difference is not at 40 iterations, where the first pass has not stalled yet,
but at the end: with restarts, bigblue2 converged after 153 iterations and 2 restarts, and
newblue1 after 367 and 4, where without them, stopping after 25 iterations without progress,
bigblue2 was left with 9 edges after 154 iterations, and newblue1 with 101 after 97.

- **Crowding to a higher power.** `EngineOptions::crowding` at 1, the square of how full an edge
  is, priced an edge at twice its length when full and 1.56 times when three quarters full, so
  that the edges around a congested region cost about as much as its middle, and nets only left
  the edges that overflowed for the full ones next to them. NTHU-Route 2.0's penalty is the
  fifth power of how full an edge would be, and NCTU-GR 2.0's a logistic curve that rises within
  the last ten or so tracks. `EngineOptions::crowding_exponent`, added for this, makes the
  power anything: at the 8th, an edge costs 1 + 0.004 `crowding` times its length when half
  full, 1 + 0.1 `crowding` at three quarters, 1 + 0.43 `crowding` at 90% and 1 + `crowding`
  when full. Higher factors kept
  helping up to 16; 32 was no better, and nor were the 12th and 16th powers.
- **Boxes.** A box that grows by 5 cells each iteration that its net overflows covers most of
  the design after 40 iterations: on newblue1, the expansions of an iteration grew tenfold from
  the fifth to the fortieth. Capping the growth at 40 cells took the fortieth's from 692 to 95
  million, with about as much overflow: 285 edges against 295, and 112 against 123 on
  bigblue2. A cap of 100 changed nothing either.
- **Longest first.** The engine routes nets in the order given. NTHU-Route 2.0 routes the nets
  with the largest boxes first, since they have the most ways round congestion and the smallest
  can then keep the short ways. On its own, that changed little; with crowding at 16 it took
  newblue1 from 164 edges to 131 at 40 iterations and bigblue2 from 44 to 36.
- **Starting again.** Each pass of negotiation still stalls, with a few to a hundred edges one
  net over, which it moves about rather than settles: the present factor has long reached its
  cap, so that no net will overflow anything it can go round, and history only grows on the
  edges that overflow. A new pass from the best routes so far, with the history that the last
  one learned but the present factor back at 0.5, lets nets share edges again for a few
  iterations and settle where history now sends them. The least overflow on the plane of each
  pass, in tracks, was 17, 1 and 0 on bigblue2, of 95, 44 and 13 iterations, and 101, 15, 10, 5
  and 0 on newblue1, of 87, 77, 80, 65 and 57. Starting again without the history was no
  better than going on: the first iteration of the new pass had as much overflow as the first
  of all.

On adaptec2, newblue2 and adaptec4, which converged before, wirelength by step, each against
the one before:

| Version | adaptec2 WL | Vias | newblue2 WL | Vias | adaptec4 WL | Vias |
| --- | --- | --- | --- | --- | --- | --- |
| The first results' defaults | 5 375 045 | 2 002 824 | 7 722 773 | 3 034 301 | 12 384 400 | 3 338 413 |
| Steiner trees for pattern routes | 5 358 393 | 1 999 655 | 7 677 366 | 3 026 504 | 12 310 300 | 3 331 853 |
| Layers: a full layer costs a twentieth of a via more | 5 287 138 | 1 928 400 | 7 569 356 | 2 918 494 | 12 178 628 | 3 200 181 |
| Z shapes | 5 274 978 | 1 923 788 | 7 560 877 | 2 920 189 | 12 158 360 | 3 197 380 |
| `crowding` 10 to the 8th, boxes capped | 5 414 073 | 1 932 826 | 7 637 333 | 2 937 570 | 12 257 974 | 3 210 822 |
| Refinement | 5 270 547 | 1 931 683 | 7 563 278 | 2 930 187 |  |  |
| The defaults, `crowding` 16, longest first | 5 284 412 | 1 935 715 | 7 565 097 | 2 929 627 | 12 144 323 | 3 204 038 |

- **Steiner trees.** Pattern routes along a rectilinear Steiner tree from the iterated 1-Steiner
  heuristic, for nets of up to 64 pins, rather than a spanning tree: 0.3% to 0.6% less
  wirelength, nearly all of it wire. Most nets have two or three pins, and three take the exact
  tree through their median; the spanning tree's Ls already overlapped where they could.
- **Layers.** Layer assignment gave a net the fewest vias that fit, and between layers that took
  as many, the lowest. Pricing each edge on a layer at a twentieth of a via times the square
  of how full it would be makes a net take the emptier, which leaves room on the lower layers
  for the nets after it: 3.6% to 4.0% fewer vias. From a seventieth to a quarter of a via did
  the same, to 0.2%; a whole via was 3% worse on adaptec4, and four far worse.
- **Z shapes** with two turns, where both Ls are dearer: 0.1% to 0.2% less.
- **Crowding** at 10 to the 8th costs wirelength where it is not needed: nets keep away from
  nearly full edges by going round, 4% more wire on adaptec2.
- **Refinement** takes it back. Once the plane is free of overflow, the nets longer than a
  Steiner tree of their pins, the ones that went round something, are routed again once each,
  in a fresh engine seeded with every route (`Engine::rip_up`, added for this), at a present
  factor of 1000 so that nothing overflows, with crowding at 1. With crowding at 0 instead,
  nets crowd the edges the others left and take more vias; at 2 or 4, no better. Routing every
  net again, as a first try did with the old defaults, made the routes of nets that were never
  routed again by the engine into maze routes, with more turns than their Ls: 1% less wire and
  2% more vias.

#### What was tried and not kept

Before the second round, on newblue1, bigblue2 and newblue5, each against the defaults of the
time at the same iteration, none of which got any of the three closer to no overflow:

- **Ripping up the nets on full edges near the overflow as well**, within 1 or 2 cells, through
  an engine method added for it and taken out again, so that they could make room, with their
  boxes grown: 430 edges overflowing on newblue1 after 13 iterations against 482, 108 on
  bigblue2 after 29 against 135, and 66 on newblue5 after 16 against 51, for 1.5 to 2 times the
  time of an iteration.
- **Raising `crowding` each iteration**, by 1.3 or 1.5 times from the third or fifth, as
  NTHU-Route 2.0 lowers its base cost over the iterations so that nets take longer ways round
  full regions: 339 and 303 edges overflowing on newblue1 after 27 and 28 iterations against 368
  and 375, and with ripping up as well, on bigblue2, 144 after 17 against 159.
- **Batches of 1**, so that nets on the same edge do not all move at once, were not the cure
  either: before crowding, with batches of 1 throughout, adaptec2 still had 3 edges
  overflowing after 100 iterations.

In the second round, on bigblue2 and newblue1, edges overflowing after 40 iterations, against
the same options without the change:

| Tried | bigblue2 | newblue1 | Against |
| --- | --- | --- | --- |
| Ripping up all nets on full edges within 2 cells of an overflow, and a cost added to the edges within 5 | 98 | | 94 |
| Virtual capacity, as FastRoute 4.1: holding back up to 2 tracks of an edge each iteration it is full | over 5 000 by the 11th | over 2 500 by the 16th | 94, 322 |
| Present factor capped at 3, 10 or 50, not 1000 | 152, 102, 108 | 292, 299, 285 | 123, 295 |
| The same, at 30, 100 or 300, with `crowding` 10 to the 8th | 47, 44, 45 | 153, 146, 148 | 53, 154 |
| Nets in random order each iteration (`shuffle_seed`) | 104 | 284 | 94, 322 |
| Only the overflowing part of a net routed again (`incremental_reroute`) | 101 | 330 | 123, 295 |
| The same, with `crowding` 10 to the 8th and boxes in proportion | 52 | 157 | 44, 141 |
| `history_factor` 0.3, with `crowding` 6 to the 8th | 97 | 276 | 54, 198 |
| `history_factor` 2, with `crowding` 10 to the 8th, after 150 iterations | 21 | 140 | 14, 104 |
| `crowding` raised from 1 by 1.3 times an iteration to 10, to the 8th, after 35 | 84 at 30 | 218 | 50 at 30, 164 |
| Ripping up all nets on full edges within 1 cell, with `crowding` 6 to the 8th | 47 | 173 | 54, 198 |
| The same with `crowding` 10, after 150 iterations | 22 | 96 | 14, 104 |
| Boxes a quarter of each net's half perimeter beyond its pins, growing by a tenth | 44 | 141 | 53, 154 |
| Batches of 1, with `crowding` 10 to the 8th | 46 | 147 | 53, 154 |

- **Ripping up the nets around an overflow**, now that full edges are dear enough for them to
  leave, helped a little at 40 iterations, and after 150 left more on bigblue2 and less on
  newblue1, for twice the time of an iteration. Pricing the region around an overflow rather
  than its edges did nothing.
- **Virtual capacity** reserves tracks on edges that overflow, so that the engine sees them as
  overused before they are: on regions full to 96%, holding back even 2 tracks made the problem
  infeasible, and negotiation diverged.
- **A lower present factor**, so that a net would sooner overflow an edge than go far round it,
  as with NCTU-GR 2.0's bound on a net's length, slowed convergence, and with steep crowding
  made no difference.
- **Raising crowding over the iterations**, with the 8th power too, was worse at every
  iteration than the final value from the start.
- **Boxes in proportion** to each net's size, as a stand-in for NCTU-GR 2.0's bounded-length
  maze routing, gained a little, as much as longest first, which was kept instead; the two
  were not tried together.
- **Batches of 1**: NCTU-GR 2.0 reports that its parallel router does not converge on newblue1
  where its sequential one does, at the 83rd iteration. Here batches of 32 did about as well as
  1, in up to 2.5 times less time: a route of a batch that uses an edge that an earlier one of
  the batch filled is dropped and routed again.
- **Two-pin nets.** NTHU-Route 2.0 and NCTU-GR 2.0 rip up and route again two-pin pieces of
  each net, along its spanning or Steiner tree, rather than whole nets. Handing the engine those
  pieces as nets of their own made adaptec2, which converges in 9 iterations, stall with 3 000
  edges over: the engine counts a net once on an edge, and the pieces of one net that share an
  edge, near a pin with several, as many times as there are of them, and those routers count
  them once. Doing it here would take a net's pieces sharing what they hold, in the engine.
- **Starting again without the history**, or every 6 iterations without progress rather than
  10 or 15, did no better; with 10, newblue1 was left with 4 to 6 edges after 350 to 400
  iterations.

#### CPU time, and the four that no router solves

The third round, from `732bf59`, on the CPU time that steep crowding had added, and on the
four designs left with overflow. Expansions are the nodes that searches expanded, over the
whole run, which no machine or thread count changes; times are in the results above.

**Layers on edges short on every layer.** Nearly half of newblue3's 3D overflow was layer
assignment's own doing. On the second round's routes, every unit of its 65 046 was on an edge
that the plane left over capacity, where the least that the layers can have is twice the
plane's 18 714 tracks, 37 428; and the 27 618 units more were exactly the capacity left unused
on those same edges, on layers that had room while others overflowed. On one edge, all 88 nets
were on layer 3, 70 over its 18 tracks, with layers 1 and 5, of 4 and 18 tracks, empty. Layer assignment raised the
price of every edge over capacity on its layer after each pass, by a quarter of an overflow for
each unit over. On an edge that the plane left short on every layer, that only sent its nets
to the layers not raised yet, and once one was dear enough, overflowing another layer cost less
than taking one that was empty. Now only an edge that overflows on a layer while another layer
of it has room, an overflow that layers given again can take away, is raised, and only the nets
on such edges are given layers again. With 15 iterations of negotiation, and 19 700 tracks over
on the plane, newblue3 went from 71 006 to 39 390, a little under twice the plane's thanks to
odd capacities. bigblue4, newblue4 and newblue7 already had exactly twice the plane's, and the
twelve that converge never overflow a layer, so nothing else changed.

**Mild crowding first.** Crowding at 16 to the 8th power had been chosen for the three
designs that stalled, and it costs the others: nets keep away from edges that are nearly full
by going round, which makes each search expand more, and the refinement does not take all the
detours back. On adaptec2, from the second round's defaults:

| Crowding | Iterations | Expansions | WL | Vias |
| --- | --- | --- | --- | --- |
| 16 to the 8th, the second round's defaults | 9 | 0.97e9 | 5 284 412 | 1 935 715 |
| 4 to the 8th | 9 | 0.33e9 | 5 245 937 | 1 926 368 |
| 1 to the 8th | 10 | 0.19e9 | 5 227 458 | 1 920 551 |
| 1 to the 2nd, the first results' | 13 | 0.34e9 | 5 234 495 | 1 919 101 |

But mild crowding throughout does not make newblue1 converge: with 4 to the 8th, it was left
with 12 after 430 iterations and 5 restarts. So negotiation now makes a first try at crowding 1
(`AsicOptions::first_crowding`), and gives it up as soon as it stops halving the overflow, 3
iterations without (`AsicOptions::first_stall_iterations`), unless the overflow is down to a
thousandth of its first iteration's. A design that the first try does not settle is then
negotiated from the start exactly as before: from its pattern routes, with no history, and
boxes as they were, at crowding 16, so that it converges as it did. How the first try ends
mattered more than anything else:

| First try ends, and then | adaptec5 | bigblue2 | bigblue3 | newblue1 | newblue5 |
| --- | --- | --- | --- | --- | --- |
| No first try: the second round | 7.08e9 | 6.42e9 | 2.37e9 | 22.97e9 | 5.79e9 |
| After 15 iterations without 1% less, restart at 16 with its history | 3.91e9 | 4.48e9 | 1.31e9 | TOF 4 after 500 | 9.50e9 |
| After 10 without halving, the same | 3.91e9 | 4.96e9 | 2.53e9 | TOF 16 after 326 | 7.34e9 |
| After 5 without halving, the same | 8.51e9 | 5.31e9 | 2.08e9 | | 6.25e9 |
| After 10 without halving, the same, the present factor kept at its cap | | | 3.31e9 | | |
| After 3 without halving, above a thousandth: from the start, the defaults | 3.91e9 | 7.10e9 | 3.26e9 | 23.29e9 | 8.10e9 |

Going on from the first try, with what it learned, is cheapest where it works, but on newblue1
it never converged: the first try's routes and history are not where the steep crowding finds
its way out, and a first try that keeps making a little progress uses up the iterations. A
restart at the steep crowding with the present factor back at 0.5 also costs a burst of
expansions, as nets that may share again leave the edges that are nearly full: adaptec5, left
with 1 track over by the first try, took 4.6e9 expansions more to settle it. Starting again
from scratch spends the first try's iterations for nothing on the designs that it does not
settle, but changes nothing else: bigblue2, bigblue3, newblue1 and newblue5 end with the same
routes as in the second round, after 0.3e9 to 2.3e9 expansions more, and adaptec5 is the only
one of the five that the first try settles. adaptec1 to 4, bigblue1, newblue2 and newblue6
converge in it.

**Overflow that is left.** Where negotiation stops with overflow, the engine prices an edge
over capacity by how far over it would be, which spreads overflow about until none is left;
on a design that cannot be routed without, that sends nets round the edges most over through
others that then overflow as well. `EngineOptions::marginal_overuse`, added for this, prices
instead only what a net would add, 1 for each edge it would take beyond capacity. Once
negotiation has stopped with overflow left, the nets on it are routed again with that, at a
present factor of 1000 and no history, for as long as each iteration brings the overflow on
the plane down, up to 10 (`AsicOptions::overflow_passes`). On newblue3, whose overflow is
mostly nets that must leave regions of no capacity, it took the plane from 18 714 tracks over
to 15 920 in 7 iterations, and the 3D overflow, with the layers above, from 65 046 to 31 832,
0.4% above SGR's 31 710 and 2.3% above the contest's best, 31 106. On bigblue4 and newblue4, whose few hundred
units of overflow are spread over a few edges, its first iteration left more than it started
from, 202 tracks against 160 and 95 against 90, and the routes were kept as they were: a net
of many terminals is routed again one terminal at a time, which need not find its old tree,
and nets of a batch see the same state, so the total can rise. Batches of 1 did no better on
newblue4: 99 against 90. On newblue7 too, 91 against 83, so that the three are left where
negotiation left them: bigblue4 with 320, newblue4 with 180 and newblue7 with 166.

**Searches.** perf, which works here for user space at least, put the time of adaptec2's first
iterations, on one thread, in pricing steps, 30%; the heap, 29%; offering a node to the
search, which is mostly its lower bound over the search's targets, 22%; and the rest of the
search, 6%. A search has an average of 130 expansions, and twice as many pushes: a net's
search starts from every node of its tree so far, and ends at any access point of a terminal
not yet reached, and multi-pin nets take most of the time, nets of 11 to 100 pins 10 of the
first 17 seconds of searches. Three changes leave every result as it was: the plane divides
its numbers of cells by a multiplication rather than a division, about 6% of adaptec2's time
on a busy machine; and `RoutingSpace::lower_bound_to_targets`, a new method with a default,
through which the plane finds the cell of a search's node once rather than once for each
target, skips the second access point of each terminal, whose bound is the same, and keeps the
least distance in integers, with the 8th power by three squarings rather than a loop, 6% of
the instructions of a run of newblue2 on one thread.

Tried and not kept, in this round:

- **Routing again only the overflowing part of a net** (`incremental_reroute`) in the first
  try: on adaptec2, 0.16e9 expansions rather than 0.19e9, for 0.06% more wirelength. With the
  steep crowding it had slowed convergence before, and it was not measured further.
- **Crowding 1 throughout**, with restarts: bigblue2 converged after 228 iterations, with
  1.4% less wirelength, but newblue1 does not converge with mild crowding.

### What this does not show

- **One run each, with one set of options**, as the contest required. Batches change results,
  so another batch size or thread count than these would give other numbers, by about as much
  as the differences between options above.
- **The options were tuned on these designs.** The crowding, the cap on boxes and when to start
  again were chosen on bigblue2, newblue1 and newblue5, and the last of them, 15 iterations
  without progress rather than 10, because 10 left newblue1 with 6 edges one net over. There is
  no other set of designs of this kind to check them on, and the contests' routers were tuned
  on these as well. The third round's first try, 3 iterations without halving above a
  thousandth, was chosen on adaptec5, bigblue2, bigblue3, newblue1 and newblue5, and bigblue3,
  which the first try took down to 12 tracks over, a little above its floor of 10, would have
  been cheaper with a floor twice as high, and newblue5 dearer.
- **Runtimes are not comparable** with the published ones: other machines, years apart, and
  mostly one thread against many here. What can be compared is the work: the expansions are in
  the logs.
- **2007 is scored by eval2008.pl's rules** with 2007's via cost, not by `grc_eval`, whose other
  rules, such as counting a via's usage, were not reproduced.
- **No detailed routing**, and nothing of what makes a global route good for it beyond overflow
  and length: pin access, track assignment, local congestion inside cells.
- **`EngineOptions::crowding`, `crowding_exponent`, `marginal_overuse` and `Engine::rip_up`
  were made for this domain.** They are general, and off unless set, but have only been
  measured here. So was `RoutingSpace::lower_bound_to_targets`, whose default leaves every
  other space as it was.
- **Times this round are rough**: the machine was shared with other work throughout, at a load
  of 15 to 180, and this work was held to 16 threads at once. The one-thread comparison of the
  two rounds ran both side by side at a load of about 10.
