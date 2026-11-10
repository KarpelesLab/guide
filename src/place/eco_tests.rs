//! Tests and measurements of incremental placement.
//!
//! The measurement is `#[ignore]`d: `docs/benchmarks.md` has its results and how to run it.

use std::time::Instant;

use super::*;
use crate::place::synth::{self, Generated, PekoOptions};
use crate::rng::Rng;

fn assert_legal(problem: &PlacementProblem, placement: &Placement) {
    let violations = check(problem, placement);
    assert!(violations.is_empty(), "{violations:?}");
}

fn analytic(problem: &PlacementProblem, seed: u64) -> PlaceOutcome {
    let options = AnalyticOptions {
        seed,
        ..AnalyticOptions::default()
    };
    place_analytic(problem, Wirelength, options)
        .expect("placement")
        .0
}

/// Every `n`th item, starting at `offset`, among those that can move and are in no macro.
fn every_nth_item(problem: &PlacementProblem, fraction: f64, seed: u64) -> Vec<bool> {
    let in_macro: Vec<bool> = {
        let mut flags = vec![false; problem.items().len()];
        for members in problem.macros() {
            for &(item, _) in members {
                flags[item.index()] = true;
            }
        }
        flags
    };
    let mut candidates: Vec<usize> = (0..problem.items().len())
        .filter(|&i| problem.items()[i].is_movable() && !in_macro[i])
        .collect();
    Rng::new(seed).shuffle(&mut candidates);
    let count = ((candidates.len() as f64 * fraction).round() as usize).max(1);
    let mut keep = vec![true; problem.items().len()];
    for &i in candidates.iter().take(count) {
        keep[i] = false;
    }
    keep
}

fn every_nth_net(problem: &PlacementProblem, fraction: f64, seed: u64) -> Vec<bool> {
    let mut nets: Vec<usize> = (0..problem.net_count()).collect();
    Rng::new(seed).shuffle(&mut nets);
    let count = ((nets.len() as f64 * fraction).round() as usize).max(1);
    let mut keep = vec![true; problem.net_count()];
    for &n in nets.iter().take(count) {
        keep[n] = false;
    }
    keep
}

#[test]
fn an_unchanged_placement_stays_as_it_is_without_annealing() {
    let generated = synth::peko(PekoOptions::new(8, 8, 5));
    let problem = &generated.problem;
    let before = analytic(problem, 1);
    let options = IncrementalOptions {
        anneal: None,
        ..IncrementalOptions::default()
    };
    let after = place_incremental(problem, Wirelength, &Prior::all(&before.placement), options)
        .expect("placement");
    assert_eq!(after.outcome.placement, before.placement);
    assert_eq!(after.change.moved, 0);
    assert_eq!(after.placed_by_legalization, 0);
}

#[test]
fn an_unchanged_placement_hardly_moves_with_annealing() {
    for generated in [
        synth::peko(PekoOptions::new(8, 8, 5)),
        synth::free_grid(5, 5, 10, 5, 20, 3),
        synth::grid_with_macros(6, 8, 3, 3),
    ] {
        let problem = &generated.problem;
        let before = analytic(problem, 1);
        let after = place_incremental(
            problem,
            Wirelength,
            &Prior::all(&before.placement),
            IncrementalOptions::default(),
        )
        .expect("placement");
        assert_legal(problem, &after.outcome.placement);
        assert!(after.outcome.wirelength <= before.wirelength);
        assert!(after.change.moved_fraction() < 0.2, "{:?}", after.change);
    }
}

#[test]
fn added_items_are_placed_legally_and_the_rest_mostly_stay() {
    let cases: Vec<Generated> = vec![
        synth::peko(PekoOptions::new(10, 10, 5)),
        synth::peko(PekoOptions {
            pads: true,
            typed_columns: Some(4),
            ..PekoOptions::new(10, 10, 6)
        }),
        synth::grid_with_macros(6, 8, 3, 3),
        synth::free_grid(5, 5, 10, 5, 20, 3),
    ];
    for generated in cases {
        let problem = &generated.problem;
        let keep_items = every_nth_item(problem, 0.05, 9);
        let keep_nets = vec![true; problem.net_count()];
        let restricted = restrict(problem, &keep_items, &keep_nets);
        let before = analytic(&restricted.problem, 1);
        let priors = restricted.priors(&before.placement);
        let after = place_incremental(problem, Wirelength, &priors, IncrementalOptions::default())
            .expect("placement");
        assert_legal(problem, &after.outcome.placement);
        let added = keep_items.iter().filter(|&&keep| !keep).count() as u32;
        assert_eq!(after.change.added, added);
        assert!(after.change.moved_fraction() < 0.3, "{:?}", after.change);

        let again = place_incremental(problem, Wirelength, &priors, IncrementalOptions::default())
            .expect("placement");
        assert_eq!(again, after);
    }
}

#[test]
fn a_high_displacement_cost_keeps_every_item_with_a_prior() {
    let generated = synth::peko(PekoOptions::new(10, 10, 5));
    let problem = &generated.problem;
    let keep_items = every_nth_item(problem, 0.05, 4);
    let restricted = restrict(problem, &keep_items, &vec![true; problem.net_count()]);
    let before = analytic(&restricted.problem, 1);
    let options = IncrementalOptions {
        displacement_permille: 1_000_000,
        ..IncrementalOptions::default()
    };
    let after = place_incremental(
        problem,
        Wirelength,
        &restricted.priors(&before.placement),
        options,
    )
    .expect("placement");
    assert_legal(problem, &after.outcome.placement);
    assert_eq!(after.change.moved, 0, "{:?}", after.change);
}

#[test]
fn a_prior_site_that_is_gone_is_replaced_by_the_nearest() {
    let sites: Vec<Site> = (0..4).map(|x| Site::new(Point::new(x, 0), 0)).collect();
    let mut problem = PlacementProblem::new(Layout::Sites(sites));
    let a = problem.add_item(Item::new());
    let b = problem.add_item(Item::new());
    problem.add_net(1, &[Pin::new(a), Pin::new(b)]);
    // Both say they were on site 1: the first keeps it, the second goes next to it.
    let prior = Prior {
        origin: Point::new(1, 0),
        rotation: 0,
        site: Some(SiteId(1)),
    };
    let options = IncrementalOptions {
        anneal: None,
        ..IncrementalOptions::default()
    };
    let after = place_incremental(&problem, Wirelength, &[Some(prior), Some(prior)], options)
        .expect("placement");
    assert_legal(&problem, &after.outcome.placement);
    assert_eq!(after.outcome.placement.site(a), Some(SiteId(1)));
    assert_eq!(after.outcome.placement.site(b), Some(SiteId(0)));
    assert_eq!(
        (after.kept_by_legalization, after.placed_by_legalization),
        (1, 1)
    );
}

#[test]
fn restricting_to_everything_changes_nothing() {
    let generated = synth::grid_with_macros(6, 8, 3, 3);
    let problem = &generated.problem;
    let restricted = restrict(
        problem,
        &vec![true; problem.items().len()],
        &vec![true; problem.net_count()],
    );
    assert_eq!(&restricted.problem, problem);
}

// The measurement.

/// A family of generated problems, by name, made from a seed.
type Family = (&'static str, Box<dyn Fn(u64) -> Generated>);

/// What one placement of the changed problem gave.
struct Measured {
    wirelength: u64,
    change: PlacementChange,
    seconds: f64,
}

/// How a changed problem is placed in the measurement.
#[derive(Clone, Copy)]
enum Method {
    /// From scratch, with a seed.
    Scratch(u64),
    /// Incrementally, with a displacement factor.
    Incremental(u64),
}

fn measure_one(problem: &PlacementProblem, priors: &[Option<Prior>], method: Method) -> Measured {
    let started = Instant::now();
    let placement = match method {
        Method::Scratch(seed) => analytic(problem, seed).placement,
        Method::Incremental(permille) => {
            let options = IncrementalOptions {
                displacement_permille: permille,
                ..IncrementalOptions::default()
            };
            place_incremental(problem, Wirelength, priors, options)
                .expect("placement")
                .outcome
                .placement
        }
    };
    let seconds = started.elapsed().as_secs_f64();
    assert_legal(problem, &placement);
    Measured {
        wirelength: problem.wirelength(&placement),
        change: placement_change(problem, priors, &placement),
        seconds,
    }
}

/// Places generated problems after small changes, incrementally and from scratch, and prints a
/// table of what moved and what it cost. Run with
///
/// ```text
/// cargo test --release --lib place::eco_tests::measure -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a measurement, which prints a table"]
fn measure() {
    let families: Vec<Family> = vec![
        (
            "Peko 32x32",
            Box::new(|seed| synth::peko(PekoOptions::new(32, 32, seed))),
        ),
        (
            "Peko 32x32, typed, pads",
            Box::new(|seed| {
                synth::peko(PekoOptions {
                    pads: true,
                    typed_columns: Some(4),
                    ..PekoOptions::new(32, 32, seed)
                })
            }),
        ),
        (
            "Free grid 16x16, room",
            Box::new(|seed| synth::free_grid(16, 16, 10, 5, 40, seed)),
        ),
    ];
    let changes: [(&str, f64, f64); 4] = [
        ("1% items added", 0.01, 0.0),
        ("5% items added", 0.05, 0.0),
        ("1% nets added", 0.0, 0.01),
        ("5% nets added", 0.0, 0.05),
    ];
    let methods: [(&str, Method); 5] = [
        ("From scratch", Method::Scratch(0)),
        ("From scratch, another seed", Method::Scratch(100)),
        ("Incremental, 0", Method::Incremental(0)),
        ("Incremental, 300", Method::Incremental(300)),
        ("Incremental, 1000", Method::Incremental(1000)),
    ];
    println!(
        "| Problem | Change | Method | Wirelength vs scratch | Items moved | Mean displacement | Max | Time (s) |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
    for (name, make) in &families {
        for &(change, items, nets) in &changes {
            let mut sums = [(0.0, 0.0, 0.0, 0u64, 0.0); 5];
            let seeds = [1u64, 2, 3];
            for &seed in &seeds {
                let generated = make(seed);
                let problem = &generated.problem;
                let keep_items = if items > 0.0 {
                    every_nth_item(problem, items, seed)
                } else {
                    vec![true; problem.items().len()]
                };
                let keep_nets = if nets > 0.0 {
                    every_nth_net(problem, nets, seed)
                } else {
                    vec![true; problem.net_count()]
                };
                let restricted = restrict(problem, &keep_items, &keep_nets);
                let before = analytic(&restricted.problem, seed);
                let priors = restricted.priors(&before.placement);
                let mut scratch = None;
                for (slot, &(_, method)) in methods.iter().enumerate() {
                    let method = match method {
                        Method::Scratch(offset) => Method::Scratch(seed + offset),
                        other => other,
                    };
                    let measured = measure_one(problem, &priors, method);
                    let reference = *scratch.get_or_insert(measured.wirelength);
                    let sum = &mut sums[slot];
                    sum.0 += measured.wirelength as f64 / reference as f64;
                    sum.1 += measured.change.moved_fraction();
                    sum.2 += measured.change.mean_displacement();
                    sum.3 = sum.3.max(measured.change.max_displacement);
                    sum.4 += measured.seconds;
                }
            }
            let n = seeds.len() as f64;
            for (slot, (method, _)) in methods.iter().enumerate() {
                let sum = sums[slot];
                println!(
                    "| {name} | {change} | {method} | {:.3} | {:.1}% | {:.2} | {} | {:.3} |",
                    sum.0 / n,
                    100.0 * sum.1 / n,
                    sum.2 / n,
                    sum.3,
                    sum.4 / n,
                );
            }
        }
    }
}
