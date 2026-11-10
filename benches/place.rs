//! Placement on synthetic problems whose optimal wirelength is known.
//!
//! Every instance comes from `guide::place::synth` with a fixed seed. See
//! `docs/benchmarks.md` for what has been measured on them.
//!
//! ```text
//! cargo bench --bench place                            # timings, by criterion
//! GUIDE_PLACE_SWEEP=quality cargo bench --bench place  # ratios to the optimum, no criterion
//! GUIDE_PLACE_SWEEP=threads cargo bench --bench place  # the same results on 1 and 16 threads
//! ```
//!
//! The quality sweep runs one instance at a time, so that its times are fair. Wirelengths do
//! not depend on the machine and can be compared exactly.

use std::hint::black_box;
use std::thread;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group};
use guide::place::synth::{self, Generated, PekoOptions};
use guide::place::{
    AnalyticOptions, AnnealOptions, PlaceOutcome, Wirelength, anneal, check, place_analytic,
};

/// The environment variable that selects a sweep in place of the timings.
const SWEEP_VARIABLE: &str = "GUIDE_PLACE_SWEEP";

/// Seeds per family in the quality sweep.
const SEEDS: u64 = 3;

/// A family of instances: a name, and how to make one from a seed.
type Family = (&'static str, fn(u64) -> Generated);

fn peko(size: u32, pads: bool, typed_columns: Option<u32>, seed: u64) -> Generated {
    let mut options = PekoOptions::new(size, size, seed);
    options.pads = pads;
    options.typed_columns = typed_columns;
    synth::peko(options)
}

fn families() -> Vec<Family> {
    vec![
        ("Grid 16 x 16", |seed| synth::grid(16, 16, seed)),
        ("Grid 16 x 16, pads", |seed| {
            synth::grid_with_pads(16, 16, seed)
        }),
        ("Grid 32 x 32, pads", |seed| {
            synth::grid_with_pads(32, 32, seed)
        }),
        ("Grid 16 x 16, pads, macros of 4", |seed| {
            synth::grid_with_macros(16, 16, 4, seed)
        }),
        ("Chain of 200 on 16 x 16", |seed| {
            synth::chain(200, 16, 16, seed)
        }),
        ("Peko 16 x 16", |seed| peko(16, false, None, seed)),
        ("Peko 32 x 32, pads", |seed| peko(32, true, None, seed)),
        ("Peko 32 x 32, pads, typed columns", |seed| {
            peko(32, true, Some(8), seed)
        }),
        ("Free space 8 x 8, full", |seed| {
            synth::free_grid(8, 8, 10, 5, 0, seed)
        }),
        ("Free space 12 x 12, full", |seed| {
            synth::free_grid(12, 12, 10, 5, 0, seed)
        }),
        ("Free space 12 x 12, 20% wider", |seed| {
            synth::free_grid(12, 12, 10, 5, 24, seed)
        }),
    ]
}

/// One method's result on one instance.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    wirelength: u64,
    seconds: f64,
}

/// What each method made of one instance.
#[derive(Clone, Debug, PartialEq)]
struct Results {
    optimum: u64,
    items: usize,
    nets: usize,
    annealed: Run,
    annealed_long: Run,
    legalized: u64,
    analytic: Run,
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed().as_secs_f64())
}

fn checked(generated: &Generated, outcome: PlaceOutcome) -> u64 {
    let violations = check(&generated.problem, &outcome.placement);
    assert!(violations.is_empty(), "illegal: {violations:?}");
    assert!(outcome.wirelength >= generated.optimum, "below the optimum");
    outcome.wirelength
}

fn run(generated: &Generated, seed: u64) -> Results {
    let problem = &generated.problem;
    let mut options = AnnealOptions::default();
    options.seed = seed;
    let (outcome, seconds) = timed(|| anneal(problem, Wirelength, options).expect("anneal"));
    let annealed = Run {
        wirelength: checked(generated, outcome),
        seconds,
    };
    let mut long = options;
    long.inner_num = 10.0;
    let (outcome, seconds) = timed(|| anneal(problem, Wirelength, long).expect("anneal"));
    let annealed_long = Run {
        wirelength: checked(generated, outcome),
        seconds,
    };
    let mut options = AnalyticOptions::default();
    options.seed = seed;
    let ((outcome, report), seconds) =
        timed(|| place_analytic(problem, Wirelength, options).expect("analytic"));
    let analytic = Run {
        wirelength: checked(generated, outcome),
        seconds,
    };
    Results {
        optimum: generated.optimum,
        items: problem.items().len(),
        nets: problem.net_count(),
        annealed,
        annealed_long,
        legalized: report.legalized,
        analytic,
    }
}

fn quality_sweep() {
    println!(
        "| Problem | Items | Nets | Optimum | Annealing | Annealing, 10x moves | Analytic, legalized | Analytic, detailed | Annealing s | 10x s | Analytic s |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    let mut all: Vec<Vec<f64>> = vec![Vec::new(); 4];
    for (name, make) in families() {
        let results: Vec<Results> = (1..=SEEDS).map(|seed| run(&make(seed), seed)).collect();
        let ratio = |f: &dyn Fn(&Results) -> u64| -> (f64, f64) {
            let ratios: Vec<f64> = results
                .iter()
                .map(|r| f(r) as f64 / r.optimum as f64)
                .collect();
            let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
            (mean, ratios.iter().copied().fold(0.0, f64::max))
        };
        let columns = [
            ratio(&|r| r.annealed.wirelength),
            ratio(&|r| r.annealed_long.wirelength),
            ratio(&|r| r.legalized),
            ratio(&|r| r.analytic.wirelength),
        ];
        for (k, (mean, _)) in columns.iter().enumerate() {
            all[k].push(*mean);
        }
        let mean =
            |f: &dyn Fn(&Results) -> f64| results.iter().map(f).sum::<f64>() / results.len() as f64;
        let first = &results[0];
        println!(
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {:.3} | {:.2} | {:.3} |",
            first.items,
            first.nets,
            mean(&|r| r.optimum as f64).round(),
            cell(columns[0]),
            cell(columns[1]),
            cell(columns[2]),
            cell(columns[3]),
            mean(&|r| r.annealed.seconds),
            mean(&|r| r.annealed_long.seconds),
            mean(&|r| r.analytic.seconds),
        );
    }
    let geomean =
        |values: &[f64]| (values.iter().map(|v| v.ln()).sum::<f64>() / values.len() as f64).exp();
    println!(
        "| Geometric mean | | | | {:.3} | {:.3} | {:.3} | {:.3} | | | |",
        geomean(&all[0]),
        geomean(&all[1]),
        geomean(&all[2]),
        geomean(&all[3]),
    );
}

fn cell((mean, worst): (f64, f64)) -> String {
    format!("{mean:.3} ({worst:.3})")
}

/// Runs every instance on one thread, then on 16 at once, and checks that nothing changed.
fn thread_sweep() {
    let jobs: Vec<(Family, u64)> = families()
        .into_iter()
        .flat_map(|family| (1..=SEEDS).map(move |seed| (family, seed)))
        .collect();
    let wirelengths = |results: &[Results]| -> Vec<[u64; 4]> {
        results
            .iter()
            .map(|r| {
                [
                    r.annealed.wirelength,
                    r.annealed_long.wirelength,
                    r.legalized,
                    r.analytic.wirelength,
                ]
            })
            .collect()
    };
    let serial: Vec<Results> = jobs
        .iter()
        .map(|&((_, make), seed)| run(&make(seed), seed))
        .collect();
    let parallel: Vec<Results> = thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(jobs.len().div_ceil(16))
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|&((_, make), seed)| run(&make(seed), seed))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("no panic"))
            .collect()
    });
    assert_eq!(wirelengths(&serial), wirelengths(&parallel));
    println!(
        "{} runs of each method gave the same wirelengths on 1 thread and on 16.",
        jobs.len()
    );
}

fn timings(c: &mut Criterion) {
    let mut group = c.benchmark_group("place");
    for (name, generated) in [
        ("peko-16", peko(16, false, None, 1)),
        ("peko-32-pads", peko(32, true, None, 1)),
        ("free-8", synth::free_grid(8, 8, 10, 5, 0, 1)),
    ] {
        let problem = &generated.problem;
        group.bench_function(format!("anneal/{name}"), |b| {
            b.iter(|| black_box(anneal(problem, Wirelength, AnnealOptions::default())))
        });
        group.bench_function(format!("analytic/{name}"), |b| {
            b.iter(|| {
                black_box(place_analytic(
                    problem,
                    Wirelength,
                    AnalyticOptions::default(),
                ))
            })
        });
    }
    group.finish();
}

fn configured() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(200))
        .measurement_time(Duration::from_secs(2))
}

criterion_group! {
    name = benches;
    config = configured();
    targets = timings
}

fn main() {
    let Ok(sweep) = std::env::var(SWEEP_VARIABLE) else {
        benches();
        Criterion::default().configure_from_args().final_summary();
        return;
    };
    match sweep.as_str() {
        "quality" => quality_sweep(),
        "threads" => thread_sweep(),
        _ => {
            eprintln!("{SWEEP_VARIABLE} must be quality or threads, not {sweep:?}");
            std::process::exit(2);
        }
    }
}
