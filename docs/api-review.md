# API review

M10's "API review and stabilization", as of November 2026, at version 0.0.1. This takes stock
of the public API, records what was changed to make it consistent, and lists what is left for
a decision before 1.0. [stability.md](stability.md) is the proposed policy for what a release
may change once it is 1.0, which this review feeds.

The inventory comes from rustdoc's JSON output for `--all-features`, walking every public
path; the checks of derives, `#[non_exhaustive]`, fields and auto traits come from the same
data. `tests/api.rs` turns the guarantees that matter into tests.

## Inventory

Items reachable from the crate root, by top-level module, submodules included (`geom` holds
`geom::drc`, `pcb` holds `pcb::eco`, `pcb::place` and `pcb::swap`, `place` holds
`place::synth`, `mapf` holds `mapf::check` and `mapf::movingai`, `space` holds `space::graph`
and `space::grid`). Methods are public inherent methods; trait implementations aren't counted.

| Module | Feature | Structs | Enums | Functions | Traits | Constants, aliases | Methods |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `cost` | | 1 | | | | | 6 |
| `id` | | 3 | | | | | 3 |
| `net` | | 9 | 4 | | | | 24 |
| `space` | | 10 | 4 | | 5 | 1 | 41 |
| `search` | | 6 | 1 | | | | 10 |
| `engine` | | 10 | 4 | 4 | | | 23 |
| `eco` | | 2 | | 4 | | 1 | 2 |
| `steiner` | | 2 | 2 | 3 | | | |
| `timing` | | 12 | 2 | | 1 | 1 | 61 |
| `validate` | | 2 | 2 | 2 | | | 8 |
| `place` | | 29 | 6 | 15 | 3 | | 99 |
| `rng` | | 1 | | | | | 5 |
| `math` | | | | 1 | | | |
| `fpga` | `fpga` | 54 | 14 | 19 | | | 144 |
| `geom` | `pcb` | 23 | 5 | 2 | | 4 | 70 |
| `pcb` | `pcb` | 23 | 4 | 16 | | | 8 |
| `kicad` | `kicad` | 13 | 2 | 13 | | 1 | 6 |
| `dsn` | `dsn` | 9 | 3 | 2 | | | 10 |
| `mapf` | `mapf` | 10 | 6 | 7 | | | 24 |
| `asic` | `asic` | 12 | 3 | 5 | | 2 | 26 |
| **Total** | | **231** | **62** | **93** | **9** | **10** | **570** |

So 405 items and 570 methods. The core, with no feature, is 153 items; `fpga` alone is 87
items and 144 methods, the largest domain by far, because VTR's formats are modelled in full.

## What was already consistent

- **Documentation.** Every public item had documentation; `missing_docs` was a warning that
  CI's `-D warnings` made an error. It is now `deny` in `Cargo.toml`.
- **Errors.** Every error type implements `std::error::Error + Send + Sync + 'static`, and
  `Display`. Error enums are all `#[non_exhaustive]`; `kicad::KicadError` and
  `dsn::DsnError` are opaque structs with a message and a position. Each module has its own,
  and there are no `Result` aliases anywhere, which is consistent if not usual.
- **Options.** Every options struct is `#[non_exhaustive]` with `Default`, `Clone` and
  `Debug`, set by `let mut o = XOptions::default(); o.field = …;`. The exceptions are
  `timing::CriticalityOptions`, whose fields are private and checked by its constructor,
  and `place::synth::PekoOptions`, which has no `Default`, as it has no sensible size.
- **Reports and results** were `#[non_exhaustive]` in `engine`, `eco`, `place`, `fpga`,
  `kicad`, `validate` and `net`.
- **Determinism.** No `HashMap` iteration in results-affecting code, `math::exp` and
  `rng::Rng` of the crate's own, and thread counts that change nothing: as
  [architecture.md](architecture.md#decisions) says, and as tests pin.
- **Numeric types** follow [architecture.md's table](architecture.md#decisions): `i64`
  coordinates in `geom`, `place` and `asic`, `Cost` for routing costs, `u64` placement
  costs, `u32` picoseconds for delays and `u64` for path totals, `i64` slacks, `u32` ID
  newtypes, `i32` millidegrees for every rotation (`place`, `pcb::place`, `kicad`, `dsn`),
  and `u64` seeds. Exceptions are listed below.
- **polyclip** appears nowhere in the API, and a test keeps it that way.
- **`Send` and `Sync`.** Everything is both, but for `engine::Engine`, which is `Send` and
  not `Sync` (it keeps scratch buffers in `RefCell`s, and every method that routes takes
  `&mut self`), and `place::AnalyticOptions` (below).

## Changes made

Each is its own commit; those marked breaking carry a `!` and a `BREAKING CHANGE` footer.
Results are unchanged: no change touches an algorithm, and every test, golden value and
benchmark-pinned result passes as before.

1. **`pcb::Options` is `pcb::RouteOptions`** (breaking). It was the one options struct named
   `Options`, next to `EngineOptions`, `AsicOptions`, `kicad::FlowOptions` and
   `pcb::place::PlaceOptions`, and read ambiguously once imported.
2. **`# Errors` and `# Panics` sections**, and `#[must_use]` on methods that return `Self`.
   25 functions returning `Result` said when they fail only in prose, or not at all: all now
   have `# Errors`. The placement problem's accessors (`PlacementProblem::item`, `net_pins`,
   `net_weight`, `pin_position`, `net_box`, `outline_at`; `Placement::origin`, `rotation`,
   `site`, `set_site`, `set_origin`) index by ID and panic out of range: they now say so.
   Clippy's `missing_errors_doc`, `missing_panics_doc` and `return_self_not_must_use` are
   on in `Cargo.toml`, so that new API keeps to this.
3. **Crate and module examples.** The crate's documentation still said it could not route;
   it now describes the core, with an example of routing and validating two nets. `geom`,
   `kicad` and `dsn` have module-level examples (the last two of files, `no_run`); `pcb`,
   `place` and `mapf` point to their function-level ones. The package description no longer
   says "no routing yet".
4. **`doc(cfg)` on docs.rs.** docs.rs builds with `--cfg docsrs`, which turns on the nightly
   `doc_cfg` feature, so every feature-gated module, and items such as
   `pcb::Design::from_kicad`, say which feature they need.
5. **More results are `#[non_exhaustive]`** (breaking): `search::SearchStats`, `Found` and
   `SearchOutcome`, `pcb::Routed`, `asic::Evaluation` and `AsicRouting`, `mapf::Solution`
   and `mapf::Round`. So is `search::SearchLimits`, as every other options struct is.
6. **Errors give their source.** `kicad::FlowError`, `place::PlaceError`, `pcb::RouteError`
   and `drc::DrcError` wrap other errors but didn't return them from `Error::source`, as
   `fpga::LoopError`, `fpga::ImportError` and `asic::ReadError` did. `Display` is
   unchanged, so messages, the CLI's included, are the same.
7. **`Debug` for `place::Annealer`**, the one public type without it, summarized as
   `engine::Engine`'s is.
8. **`tests/api.rs`** checks at compile time, per feature, that every error type is
   `Error + Send + Sync + 'static`, that the main problem and result types are `Send + Sync`,
   that options are `Default + Clone + Debug`, and that the state machines are `Debug`.
9. **No `powi` in results-affecting code.** Rust leaves its precision unspecified, which the
   determinism rules don't allow. Its two uses, in `pcb::place` and `geom`'s exact
   comparisons, were exact anyway, and are now a product and a constant of the same value.

### Lints chosen

- `missing_docs = "deny"`: every public item is documented already, and should stay so.
- `clippy::missing_errors_doc`, `missing_panics_doc`, `return_self_not_must_use`: they are
  about the public API only, cost nothing in private code, and had 44 findings, all fixed.
- Not chosen: the rest of `clippy::pedantic`. `must_use_candidate` alone has 373 findings,
  most on getters where it adds noise; `module_name_repetitions` (40) would rename types such
  as `EngineOptions` that read well; `exhaustive_structs` and `exhaustive_enums` (118) would
  force `#[non_exhaustive]` on plain data such as `geom::Point` and ID newtypes, which users
  must be able to build. `#[non_exhaustive]` is better decided type by type, below.

## Inconsistencies left, for a decision

These are breaking, or a matter of design, and are listed in the order of how much they
matter for 1.0.

### 1. How much of the API is 1.0

405 items is a large surface to freeze. Much of it is research plumbing that the examples and
scripts use to tune things, not API a user needs:

- **Tuning knobs.** `EngineOptions` has 19 public fields, `asic::AsicOptions` 21,
  `pcb::RouteOptions` 16, `pcb::place::PlaceOptions` 16, `place::AnalyticOptions` 11. Each
  becomes a promise. Options: keep them all; mark the experimental ones `#[doc(hidden)]` or
  move them into a nested `experimental` struct that semver doesn't cover (see
  [stability.md](stability.md#what-semver-covers)); or remove the ones whose defaults have
  won.
- **Helpers that exist for the examples, benchmarks and tests**: `place::synth` (synthetic
  problems with known optima, used by `benches/place.rs` and doc examples),
  `eco::STABLE_FIRST_PRESENT` and `eco::stable_options`, `mapf::engine_options`,
  `fpga::route_seeds` and `fpga::placement_priors`, `space::graph::TimedGraph` (used by
  `benches/engine.rs`), `geom::drc::KICAD_CLEARANCE_SLACK` (used by the CLI),
  `kicad::COURTYARD_FALLBACK`, `asic::MAX_PINS`, and `math::exp`, which only exists for the
  crate's own annealer but could be useful to a consumer that needs the same determinism.
- **Plumbing that leaks**: `asic::PlanarSpace` (a space with `set_boxes` and atomics, which
  the module documents as the way to route ASIC designs with the engine directly);
  `fpga::MapLookahead`, `LocationBound`, `RrDelayBound`, `HopEnds`, `RouterCongestion` and
  `ChannelDemand`, which are what `RrGraph` and the placer return. They are needed by the
  signatures that use them, so hiding them means hiding those too.
- **`fpga` as a whole** is 87 items and 144 methods: a full model of VTR's formats. A smaller,
  stable surface (read, place, route, write) with the rest `#[doc(hidden)]` or in an
  `fpga::vtr` submodule that semver covers less strictly is one option.

### 2. Two `Point`s and two `Rect`s

`geom::Point` and `place::Point` are both `{ x: i64, y: i64 }`, and both modules have a
`Rect`, with different methods. `place` has no feature, so it can't use `geom`'s; `pcb::place`
converts between them. One `Point` and `Rect` in the core, with `geom` re-exporting them, would
remove the conversions, but changes both modules' types and methods.

### 3. Input structs with public fields

`pcb::Design` (9 fields, which grew `pairs`, `lengths` and `zones` over time),
`geom::drc::Board` and its items (`Pad`, `Track`, `Via`, `Zone`, `Keepout`), `geom::RuleSpec`
(13 fields), `pcb::NetSpec`, `pcb::CopperZone`, `kicad::KicadBoard`, `dsn::DsnBoard`,
`dsn::Session`, `asic::Design` and `fpga`'s parsed models are built or read by users with
struct expressions, so they are not `#[non_exhaustive]`, and every field added to one is a
breaking change. Options: leave them open and accept that (they are data, like `Point`);
make them `#[non_exhaustive]` with constructors, as `dsn::Session::new` and
`geom::RuleSpec::new` already are; or `#[non_exhaustive]` on the readers' outputs only
(`KicadBoard`, `DsnBoard`, `kicad::Footprint` is already), which users don't build.

### 4. Enums that may grow

Not `#[non_exhaustive]`, and matched exhaustively by the CLI: `geom::drc::Item` (pad, track,
via, zone, keepout: a board edge or a hole could join), `place::Layout` (sites or free
space), `space::grid::GridCostKind`, `pcb::ZoneConnection`, `pcb::Islands`,
`dsn::WireType`, `asic::EdgeAt`, `net::NetOutcome` (routed or unrouted). Those that model a
closed set (`dsn::Side`, `pcb::place::Side`, `place::Fixed`, `mapf::State`, `mapf::Resource`,
`space::grid::Connectivity`) can stay exhaustive.

### 5. Names used twice

`Design` (`fpga`, `pcb`, `asic`), `Placement` (`fpga`, `place`), `Pin` (`fpga`, `place`,
`asic`), `Segment` (`fpga`, `asic`), `Round` (`kicad`, `mapf`), `GraphError` (`mapf`,
`space::graph`), `Fault` (`validate`, `mapf::check`), `NetStatus` (`fpga`, `validate`),
`Side` and `Component` (`dsn`, `pcb::place`), `Source` (`kicad`, `search`). Each is clear in
its module, and the modules are rarely glob-imported together; renaming is optional.

### 6. Readers and writers

Readers take `&str` (`kicad::read`, `dsn::read`, `mapf::movingai::parse_map`) or
`impl BufRead` (`fpga::read_*`, `asic::read_design`), and are named `read`, `read_*` or
`parse_*`. Writers return a `String` (`dsn::write_session`, `kicad::replace_routing`) or take
`impl Write` (`fpga::write_*`, `asic::write_net_route`). The split follows the formats (KiCad
and DSN files are rewritten in place, and S-expressions are parsed whole; VTR's files are
large and streamed), so it is defensible; `parse_*` could become `read_*` for one name.

### 7. Smaller points

- `place::AnalyticOptions` is neither `Send` nor `Sync`, because `inflation` is
  `Option<&dyn Inflation>`. Making `Inflation: Sync` (or the field `&(dyn Inflation + Sync)`)
  would fix it, and would break implementors that use `Cell`, as a test does.
- **Units in floats.** `kicad::FlowOptions::via_mm` is millimetres in an `f64`, and
  `kicad::Score::track` nanometres in an `f64`, where lengths are `i64` nanometres
  everywhere else. FPGA tile positions are `u16` in `fpga::NodeLocation` (VTR's own range)
  where the architecture's table says `u32`, and resistances and capacitances are VTR's
  `f32`.
- **Counts and indices.** Counts are `usize` in most reports and `u32` in some (`fpga`'s
  `RouteSummary`); PCB items are referred to by `usize` positions (`Item::Track(usize)`,
  `swap::PinSwap::component`) where the core uses `u32` ID newtypes.
- **Threads** are an options field (`AsicOptions::threads`, `fpga::LoopOptions::threads`,
  `mapf::SolveOptions::threads`) in some places and an argument (`engine::route_parallel`,
  `Engine::run_parallel`, `eco::reroute_parallel`) in others.
- **`pcb::RouteError`** has one variant, `Geometry`, with routing failures reported in
  `Routed::unrouted`: fine, but `#[non_exhaustive]` matters there.
- **`kicad::place_and_route`** reports wall-clock seconds (`FlowReport::first_seconds`,
  `Round::seconds`): not a result, but not deterministic either, which stability.md has to
  say.
- **Kicad fills.** `pcb::Design::from_kicad` keeps a board's saved zone fills as fixed copper;
  the CLI tells fills from filled graphics by `kicad::Source::description`, a human-readable
  string, before routing. A field or method that says which zones are fills would let a
  library user do the same without it.
- **Panics by ID.** Accessors that take an ID return `Option` in `space`, `timing`,
  `validate`, `net` and `fpga`, and panic in `place`; both are documented now. One
  convention would be simpler.

## Missing documentation

None: every public item, field and variant is documented, `missing_docs` is denied, and every
fallible function has `# Errors`. What documentation could still use is more examples:
`fpga`'s readers and placers have none of their own (the module's covers the flow), nor do
`pcb::place`, `pcb::swap` and `pcb::eco::relieve`.
