# Stability: semver and determinism

> **Draft, awaiting a decision.** This is a proposal for M10's exit criterion, "a semver
> policy that covers result determinism". Nothing here is policy yet, and the open question
> it builds on, [architecture.md's question 5](architecture.md#open-questions), stays open
> until it is decided. Where there is a choice, the options are set out with their
> trade-offs; a recommendation is marked as one. [api-review.md](api-review.md) has the
> state of the API this would apply to.

## Until 1.0

Versions are 0.0.x, which cargo treats as incompatible with each other: `guide = "0.0.1"`
only ever resolves to 0.0.1. Any release may change anything, and
[release-plz](https://release-plz.dev/) derives versions from Conventional Commits, with a `!`
or a `BREAKING CHANGE` footer marking what breaks, so that the changelog says so. That stays
true until the API is declared stable, which is the user's decision, as is whether to pass
through 0.x releases (where `0.x.y` to `0.x.(y+1)` is compatible) before 1.0.

## What semver covers

From 1.0, a release that breaks something below is a major release; one that adds to it is a
minor release; anything else is a patch.

**Covered:**

- Every item reachable from the crate root under a documented feature: its path, its
  signature, its fields if public, its variants, and its trait implementations, including
  `Send`, `Sync`, `Clone`, `Debug`, `Default`, `PartialEq`, `Eq` and `Hash` where they exist.
  `tests/api.rs` checks the ones that matter most: every error type is
  `Error + Send + Sync + 'static`, and the main types are `Send + Sync`.
- `#[non_exhaustive]` means what Rust means by it: a new field or variant there is not a
  breaking change, and code outside the crate can't rely on there being no more.
  Removing `#[non_exhaustive]` is a minor change; adding it is a major one.
- What the documentation promises: `# Errors` (which conditions fail), `# Panics` (which
  inputs panic), invariants stated in the docs (a lower bound never overestimates; a
  validator shares no code with the router; a gap equal to the clearance is no violation),
  and the formats read and written (KiCad, DSN/SES, VTR, ISPD, MovingAI): a file that reads
  today reads in every later 1.x, and what is written is accepted by the tools it is for.
- Feature names and what each enables and implies (`kicad` implies `pcb`, and so on).
  Removing a feature, or one of the items it enables, is major; adding a feature is minor.
- The `guide` command-line tool, as its own surface: commands, options, exit statuses (0
  complete and legal, 1 incomplete or illegal, 2 a wrong command line or an unreadable input)
  and the keys of the `--json` report. Adding a command, an option or a key is minor.

**Not covered:**

- Items marked `#[doc(hidden)]`, and anything documented as experimental (see the decision
  on tuning knobs below).
- The text of `Display` and `Debug` output, error messages included, and the CLI's
  human-readable output on standard error.
- Results: routes, placements, costs, iteration counts and run time. What is promised about
  them is the subject of the next section. Quality is expected to improve between releases;
  a release that makes results worse on the published benchmarks is a regression, a bug,
  not a semver matter.
- Default values of options. Changing one changes results, so it is governed by the
  determinism policy, not by semver: code that sets every option it relies on keeps working.
- Private items, the examples in `examples/`, the scripts in `scripts/`, the benchmarks and
  `docs/`, which are research harnesses and records, and polyclip, which the API never
  exposes (so upgrading it is not a breaking change for users).

## Minimum supported Rust version

Currently 1.95, with edition 2024, checked by CI's `msrv` job. reticle and cadlab, the
expected consumers, are to move to it before adopting `guide`
([architecture.md](architecture.md#target-consumers)).

| Option | Rule | For | Against |
| --- | --- | --- | --- |
| A | Raising the MSRV is a minor release, and only to a Rust at least six months old (about four releases behind stable) | Common practice; new language features without a major release | Consumers pinned to an old toolchain need to pin `guide` too |
| B | Raising the MSRV is a major release | Strongest promise for reticle and cadlab | Majors for toolchain reasons alone; edition 2024 features would freeze at 1.95 |
| C | Track the latest stable less N | Simplest | Least predictable for consumers |

*Recommendation:* A, with the MSRV and the rule stated in the README and `rust-version`,
since the consumers control their toolchains and can move with it.

## Determinism

### What is promised today

The README's goal, and architecture.md's decisions, are already a promise within a version:

> The same input, seed and options give the same output, for any thread count and on any
> platform.

Proposed wording, making each term precise:

- **Same version**: the same published version of `guide` (and so of polyclip, which
  `Cargo.lock` or `guide`'s exact requirement fixes).
- **Same inputs**: the same values, in the same order. Nets, items, pads and files given in
  another order are other inputs. Files are the same bytes.
- **Same options**: every field, the seed included. Thread count is the exception: it is an
  option that cannot change results (`batch` size, in contrast, does).
- **Same output**: every result value, bit for bit: routes, placements, costs, statistics
  that count things (iterations, expansions, rip-ups), and files written. Not wall-clock
  times (`kicad::FlowReport::first_seconds`, `kicad::Round::seconds`, the CLI's timings),
  nor anything stopped by a time limit or a cancel flag, whose result depends on when it
  stopped. Iteration and expansion budgets are deterministic, and are the way to bound a run
  reproducibly.
- **Any platform**: every 64-bit target that Rust supports with IEEE 754 `f64`, which is
  how results-affecting floating point is written (basic operations and `sqrt` in a fixed
  order, and the crate's own `exp`, sines and cosines). CI tests Linux, Windows and macOS.
  32-bit targets are not tested, and `usize` counts could overflow sooner there.

The tests that enforce it: pinned results (golden values) for the engine, timing-driven
routing and the domains, the same results at 1 and many threads, and benchmark-pinned results
in the domain tests.

### Across versions: the open question

Architecture.md's question 5: "Same-version reproducibility is a requirement. Stability across
versions would freeze every algorithmic improvement, so the current intent is that it is not
promised, and that a seed and version together identify a result." The options:

**Option 1: no promise across versions** (the current intent). Any release may change
results; the changelog says when it does.

- For: every improvement ships at once, in a patch if it is a fix; nothing to maintain.
- Against: a user who keeps reference outputs (regression tests, a board in review, a
  reproducible build in reticle) has to pin `guide = "=x.y.z"` and regenerate them on every
  upgrade, patches included. A bug fix and a result change can't be taken separately.

**Option 2: results stable within a minor version.** Patch releases never change results;
any change of results, including a change of a default, is at least a minor release.

- For: users can take patches (security, crashes, wrong errors) without new results, and
  `~1.4` pins results. It fits release-plz: a commit that changes results is a `feat:` (or
  carries a footer, say `Results-Changed:`), which makes the release minor, and the
  changelog lists those commits. The pinned-results tests are the check: a commit that has to
  update a golden value is one that changes results.
- Against: a fix that changes results waits for, or makes, a minor release; minors become
  frequent. Needs discipline in commit messages, and pinned tests broad enough to notice.

**Option 3: results stable within a major version.** Results change only in a major release.

- For: the strongest promise; a 1.x pin is a results pin.
- Against: freezes the algorithms for the life of a major, which contradicts "result quality
  first"; improvements would wait or force frequent majors, which also break the API for
  everyone.

**Option 4: versioned behaviour.** Results may change in minors, but a caller can ask for an
earlier behaviour, through something like `EngineOptions::behaviour = Behaviour::V1_2`, kept
for the life of the major.

- For: both: improvements ship, and reproducibility is opt-in.
- Against: every change that affects results has to keep the old code path, in the engine,
  the search, the placers and every domain, with tests for each; maintenance grows with every
  release. Practical for a few knobs (defaults), not for algorithms.

*Recommendation:* option 2, with option 1's wording ("a seed and a version identify a
result") kept for anything stopped by time, and defaults treated as results. It is the
smallest promise that lets users take fixes without regenerating outputs, and
it costs a convention in commit messages rather than code.

## Decisions needed

1. Which determinism option across versions (1 to 4), which settles architecture.md's
   question 5.
2. The MSRV rule (A, B or C).
3. Whether the CLI is covered by semver as proposed, and whether its `--json` keys are.
4. Whether default option values are results (as proposed) or API.
5. How to treat tuning knobs and research helpers before 1.0: keep, `#[doc(hidden)]`, an
   `experimental` struct or module outside semver, or remove
   ([api-review.md](api-review.md#1-how-much-of-the-api-is-10)).
6. Whether to promise determinism on 32-bit targets, or say 64-bit only.
7. Whether to pass through 0.x releases before 1.0.
