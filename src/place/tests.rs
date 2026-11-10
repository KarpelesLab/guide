//! Tests of placement as a whole: legality, optimality on generated problems, determinism,
//! budgets and congestion.

use core::sync::atomic::AtomicBool;

use proptest::prelude::*;

use super::*;
use crate::id::NetId;
use crate::place::synth::{self, Generated, PekoOptions};

fn assert_legal(problem: &PlacementProblem, placement: &Placement) {
    let violations = check(problem, placement);
    assert!(violations.is_empty(), "{violations:?}");
}

fn analytic(problem: &PlacementProblem) -> (PlaceOutcome, AnalyticReport) {
    place_analytic(problem, Wirelength, AnalyticOptions::default()).expect("placement")
}

fn generated() -> Vec<(&'static str, Generated)> {
    vec![
        ("grid", synth::grid(6, 5, 1)),
        ("grid with pads", synth::grid_with_pads(6, 5, 2)),
        ("grid with macros", synth::grid_with_macros(6, 8, 3, 3)),
        ("chain", synth::chain(20, 5, 5, 4)),
        ("peko", synth::peko(PekoOptions::new(8, 8, 5))),
        (
            "peko, typed, with pads",
            synth::peko(PekoOptions {
                pads: true,
                typed_columns: Some(4),
                ..PekoOptions::new(8, 8, 6)
            }),
        ),
        ("free grid", synth::free_grid(4, 4, 10, 5, 0, 7)),
    ]
}

#[test]
fn references_are_legal_and_optimal() {
    for (name, generated) in generated() {
        assert_legal(&generated.problem, &generated.reference);
        assert_eq!(
            generated.problem.wirelength(&generated.reference),
            generated.optimum,
            "{name}"
        );
        assert!(generated.optimum > 0, "{name}");
    }
}

#[test]
fn peko_nets_are_as_short_as_their_size_allows() {
    let generated = synth::peko(PekoOptions::new(10, 10, 9));
    let problem = &generated.problem;
    let mut sizes = [0u32; 21];
    for net in 0..problem.net_count() {
        let net = NetId(net as u32);
        let d = problem.net_pins(net).len() as u64;
        sizes[d as usize] += 1;
        let bounds = problem.net_box(&generated.reference, net).expect("pins");
        assert_eq!(bounds.half_perimeter(), synth::net_lower_bound(d));
    }
    assert!(sizes[2] > sizes[3] && sizes[3] > 0, "{sizes:?}");
}

#[test]
fn lower_bounds_of_small_nets() {
    let expected = [0, 0, 1, 2, 2, 3, 3, 4, 4, 4, 5];
    for (d, &b) in expected.iter().enumerate() {
        assert_eq!(synth::net_lower_bound(d as u64), b, "{d} pins");
    }
}

#[test]
fn every_method_is_legal_and_never_beats_the_optimum() {
    for (name, generated) in generated() {
        let problem = &generated.problem;
        let annealed = anneal(problem, Wirelength, AnnealOptions::default()).expect(name);
        assert_legal(problem, &annealed.placement);
        assert_eq!(annealed.stop, PlaceStop::Converged, "{name}");
        assert!(annealed.wirelength >= generated.optimum, "{name}");
        assert_eq!(annealed.cost, annealed.wirelength, "{name}");

        let (placed, report) = analytic(problem);
        assert_legal(problem, &placed.placement);
        assert!(placed.wirelength >= generated.optimum, "{name}");
        assert!(placed.wirelength <= report.legalized, "{name}");
    }
}

#[test]
fn small_anchored_problems_reach_their_optimum() {
    let generated = synth::grid_with_pads(6, 6, 11);
    let annealed =
        anneal(&generated.problem, Wirelength, AnnealOptions::default()).expect("anneal");
    assert_eq!(annealed.wirelength, generated.optimum);
    let (placed, _) = analytic(&generated.problem);
    assert_eq!(placed.wirelength, generated.optimum);

    // A quadratic placement of a grid held by its pads is the grid itself.
    let generated = synth::grid_with_pads(12, 12, 12);
    let (placed, report) = analytic(&generated.problem);
    assert_eq!(report.legalized, generated.optimum);
    assert_eq!(placed.wirelength, generated.optimum);

    // Macros trade places with macros they land on, pushing them back the other way.
    let generated = synth::grid_with_macros(8, 8, 4, 5);
    let options = AnnealOptions {
        inner_num: 10.0,
        ..AnnealOptions::default()
    };
    let annealed = anneal(&generated.problem, Wirelength, options).expect("anneal");
    assert_eq!(annealed.wirelength, generated.optimum);
}

#[test]
fn full_free_space_legalizes_near_the_global_placement() {
    // Squares that fill the area exactly: greedy legalization leaves gaps of half a square,
    // and rows after Abacus don't.
    let generated = synth::free_grid(12, 12, 10, 5, 0, 1);
    let (placed, report) = analytic(&generated.problem);
    assert_legal(&generated.problem, &placed.placement);
    let ratio = |w: u64| w as f64 / generated.optimum as f64;
    assert!(ratio(report.legalized) < 1.7, "{}", ratio(report.legalized));
    assert!(
        ratio(placed.wirelength) < 1.3,
        "{}",
        ratio(placed.wirelength)
    );
}

#[test]
fn harder_problems_come_close() {
    let generated = synth::peko(PekoOptions {
        pads: true,
        ..PekoOptions::new(10, 10, 13)
    });
    let ratio = |w: u64| w as f64 / generated.optimum as f64;
    let annealed =
        anneal(&generated.problem, Wirelength, AnnealOptions::default()).expect("anneal");
    assert!(
        ratio(annealed.wirelength) < 1.6,
        "{}",
        ratio(annealed.wirelength)
    );
    let (placed, _) = analytic(&generated.problem);
    assert!(
        ratio(placed.wirelength) < 1.4,
        "{}",
        ratio(placed.wirelength)
    );
}

#[test]
fn results_depend_only_on_the_seed() {
    for (name, generated) in generated() {
        let problem = &generated.problem;
        let options = AnnealOptions {
            seed: 99,
            ..AnnealOptions::default()
        };
        let a = anneal(problem, Wirelength, options).expect(name);
        let b = anneal(problem, Wirelength, options).expect(name);
        assert_eq!(a, b, "{name}");
        let (c, r) = analytic(problem);
        let (d, s) = analytic(problem);
        assert_eq!(c, d, "{name}");
        assert_eq!(r, s, "{name}");
    }
    // And the seed matters.
    let generated = synth::peko(PekoOptions::new(8, 8, 1));
    let seeded = |seed| {
        anneal(
            &generated.problem,
            Wirelength,
            AnnealOptions {
                seed,
                ..AnnealOptions::default()
            },
        )
        .expect("anneal")
        .placement
    };
    assert_ne!(seeded(1), seeded(2));
}

/// Regression values: a change here means results changed, which has to be deliberate.
#[test]
fn results_have_not_changed() {
    let generated = synth::peko(PekoOptions::new(8, 8, 1));
    let annealed =
        anneal(&generated.problem, Wirelength, AnnealOptions::default()).expect("anneal");
    let (placed, report) = analytic(&generated.problem);
    assert_eq!(
        (
            generated.optimum,
            annealed.wirelength,
            annealed.moves,
            report.legalized,
            placed.wirelength
        ),
        (112, 142, 23360, 160, 127)
    );
}

#[test]
fn stepping_reports_each_temperature_and_ends() {
    let generated = synth::grid_with_pads(5, 5, 3);
    let problem = &generated.problem;
    let n = problem.items().len();
    let start = legalize(problem, &vec![Point::default(); n], &vec![0; n]).expect("legal");
    let mut annealer =
        Annealer::new(problem, Wirelength, start, AnnealOptions::default()).expect("annealer");
    let mut last = None;
    while let Some(report) = annealer.step() {
        assert_legal(problem, annealer.placement());
        assert_eq!(report.cost, annealer.cost());
        assert!(report.best_cost <= report.cost);
        assert!(report.accepted <= report.moves);
        last = Some(report);
    }
    let last = last.expect("at least one temperature");
    assert_eq!(last.temperature, 0.0);
    assert_eq!(annealer.stop_reason(), Some(PlaceStop::Converged));
    assert_eq!(annealer.reports().len() as u32, last.index);
    assert!(annealer.step().is_none());
}

#[test]
fn budgets_and_cancellation_stop_with_a_legal_result() {
    let generated = synth::peko(PekoOptions::new(8, 8, 3));
    let problem = &generated.problem;
    let budget = anneal(
        problem,
        Wirelength,
        AnnealOptions {
            max_moves: 1000,
            ..AnnealOptions::default()
        },
    )
    .expect("anneal");
    assert_eq!(budget.stop, PlaceStop::BudgetExhausted);
    assert_eq!(budget.moves, 1000);
    assert_legal(problem, &budget.placement);
    assert_eq!(problem.wirelength(&budget.placement), budget.wirelength);

    let flag = AtomicBool::new(true);
    let cancelled = anneal(
        problem,
        Wirelength,
        AnnealOptions {
            cancel: Some(&flag),
            ..AnnealOptions::default()
        },
    )
    .expect("anneal");
    assert_eq!(cancelled.stop, PlaceStop::Cancelled);
    assert_eq!(cancelled.moves, 0);
    assert_legal(problem, &cancelled.placement);

    let (placed, report) = place_analytic(
        problem,
        Wirelength,
        AnalyticOptions {
            cancel: Some(&flag),
            ..AnalyticOptions::default()
        },
    )
    .expect("analytic");
    assert_eq!(report.iterations, 0);
    assert_eq!(placed.stop, PlaceStop::Cancelled);
    assert_legal(problem, &placed.placement);
}

#[test]
fn an_illegal_start_is_refused() {
    let generated = synth::grid(3, 3, 1);
    let start = Placement::new(&generated.problem);
    assert!(matches!(
        Annealer::new(
            &generated.problem,
            Wirelength,
            start,
            AnnealOptions::default()
        ),
        Err(PlaceError::IllegalStart(Violation::NotOnSite(_)))
    ));
}

fn row_of_sites(n: i64, kind: impl Fn(i64) -> u32) -> Layout {
    Layout::Sites(
        (0..n)
            .map(|x| Site::new(Point::new(x, 0), kind(x)))
            .collect(),
    )
}

#[test]
fn malformed_problems_are_refused() {
    let mut problem = PlacementProblem::new(row_of_sites(2, |_| 0));
    for _ in 0..3 {
        problem.add_item(Item::new());
    }
    assert_eq!(
        problem.validate(),
        Err(ProblemError::NotEnoughSites {
            kind: 0,
            items: 3,
            sites: 2
        })
    );
    assert!(matches!(
        anneal(&problem, Wirelength, AnnealOptions::default()),
        Err(PlaceError::Problem(ProblemError::NotEnoughSites { .. }))
    ));

    let mut problem = PlacementProblem::new(row_of_sites(3, |_| 0));
    let a = problem.add_item(Item::new().rotations(&[0, 45_000]));
    assert_eq!(
        problem.validate(),
        Err(ProblemError::UnsupportedRotation {
            item: a,
            rotation: 45_000
        })
    );

    let mut problem = PlacementProblem::new(row_of_sites(3, |x| x as u32));
    problem.add_item(Item::new().kind(0));
    let b = problem.add_item(Item::new().kind(1).fixed_at_site(SiteId(0)));
    assert_eq!(problem.validate(), Err(ProblemError::BadFixedLocation(b)));

    let mut problem = PlacementProblem::new(row_of_sites(3, |_| 0));
    let a = problem.add_item(Item::new());
    problem.add_macro(&[(a, Point::new(0, 0))]);
    problem.add_macro(&[(a, Point::new(0, 0))]);
    assert_eq!(problem.validate(), Err(ProblemError::InTwoMacros(a)));

    let mut problem = PlacementProblem::new(row_of_sites(3, |_| 0));
    problem.add_net(1, &[Pin::new(ItemId(4))]);
    assert_eq!(problem.validate(), Err(ProblemError::UnknownReference));

    let problem = PlacementProblem::new(Layout::FreeSpace {
        area: Rect::with_size(0, 0, 10, 10),
        pitch: 0,
    });
    assert_eq!(problem.validate(), Err(ProblemError::BadFreeSpace));
}

#[test]
fn the_checker_finds_each_kind_of_violation() {
    let mut problem = PlacementProblem::new(row_of_sites(4, |x| u32::from(x == 3)));
    let region = problem.add_region(Rect::new(Point::new(0, 0), Point::new(1, 0)));
    let a = problem.add_item(Item::new().region(region));
    let b = problem.add_item(Item::new().fixed_at_site(SiteId(2)));
    let c = problem.add_item(Item::new());
    let d = problem.add_item(Item::new());
    problem.add_macro(&[(c, Point::new(0, 0)), (d, Point::new(1, 0))]);
    let mut placement = Placement::new(&problem);
    placement.set_site(&problem, a, SiteId(3));
    placement.set_site(&problem, c, SiteId(0));
    placement.set_site(&problem, d, SiteId(0));
    placement.set_origin(b, Point::new(2, 0), 90_000);
    let found = check(&problem, &placement);
    for expected in [
        Violation::Moved(b),
        Violation::WrongKind(a),
        Violation::OutsideRegion(a),
        Violation::SharedSite(SiteId(0)),
        Violation::BrokenMacro(MacroId(0)),
    ] {
        assert!(found.contains(&expected), "{expected:?} in {found:?}");
    }
}

#[test]
fn regions_macros_and_rotations_are_kept() {
    // Sites: every fifth item limited to the three leftmost columns, and a macro.
    let generated = synth::peko(PekoOptions::new(9, 9, 21));
    let source = &generated.problem;
    let mut problem = PlacementProblem::new(source.layout().clone());
    let region = problem.add_region(Rect::new(Point::new(0, 0), Point::new(2, 8)));
    for (i, item) in source.items().iter().enumerate() {
        let item = if i % 5 == 0 {
            item.clone().region(region)
        } else {
            item.clone()
        };
        problem.add_item(item);
    }
    for net in 0..source.net_count() {
        let net = NetId(net as u32);
        problem.add_net(source.net_weight(net), source.net_pins(net));
    }
    problem.add_macro(&[(ItemId(1), Point::new(0, 0)), (ItemId(2), Point::new(1, 0))]);
    let annealed = anneal(&problem, Wirelength, AnnealOptions::default()).expect("anneal");
    assert_legal(&problem, &annealed.placement);
    let (placed, _) = analytic(&problem);
    assert_legal(&problem, &placed.placement);

    // Free space: rectangles that can turn, with pins off centre, a fixed obstacle, a region
    // and a macro.
    let area = Rect::with_size(0, 0, 60, 40);
    let mut problem = PlacementProblem::new(Layout::FreeSpace { area, pitch: 2 });
    let obstacle = problem.add_item(Item::new().size(20, 10).fixed_at(Point::new(20, 15), 0));
    let turnable = [0, 90_000, 180_000, 270_000];
    let items: Vec<ItemId> = (0..12)
        .map(|k| {
            problem.add_item(
                Item::new()
                    .outline(Rect::new(Point::new(-3, -2), Point::new(3 + k % 3, 2)))
                    .rotations(&turnable),
            )
        })
        .collect();
    let region = problem.add_region(Rect::with_size(0, 0, 20, 40));
    let boxed = problem.add_item(Item::new().size(4, 4).region(region));
    problem.add_macro(&[(items[0], Point::new(0, 0)), (items[1], Point::new(10, 0))]);
    for pair in items.windows(2) {
        problem.add_net(1, &[Pin::at(pair[0], 3, 1), Pin::at(pair[1], -3, 0)]);
    }
    problem.add_net(
        2,
        &[Pin::new(boxed), Pin::new(items[11]), Pin::new(obstacle)],
    );
    let annealed = anneal(&problem, Wirelength, AnnealOptions::default()).expect("anneal");
    assert_legal(&problem, &annealed.placement);
    assert!(
        items
            .iter()
            .any(|&item| annealed.placement.rotation(item) != 0)
    );
    let (placed, _) = analytic(&problem);
    assert_legal(&problem, &placed.placement);
}

#[test]
fn rudy_conserves_wire() {
    let generated = synth::peko(PekoOptions::new(10, 10, 4));
    let grid = CongestionGrid::rudy(&generated.problem, &generated.reference, 3, 10.0);
    let (nx, ny) = grid.size();
    let mut total = 0.0;
    for x in 0..nx {
        for y in 0..ny {
            total += grid.demand_at(Point::new(x as i64 * 3, y as i64 * 3));
        }
    }
    assert!((total - generated.optimum as f64).abs() < 1e-6, "{total}");
    assert!(grid.peak_utilization() > 0.0);
}

#[test]
fn congestion_moves_nets_away_from_a_hot_spot() {
    // Two items each joined to a fixed pad in the middle of a row of sites. The left of the
    // row is congested, so with congestion priced both items go right, though that costs a
    // unit of wire more than one on each side.
    let mut problem = PlacementProblem::new(row_of_sites(9, |x| u32::from(x == 4)));
    let pad = problem.add_item(Item::new().kind(1).fixed_at_site(SiteId(4)));
    let a = problem.add_item(Item::new());
    let b = problem.add_item(Item::new());
    problem.add_net(1, &[Pin::new(pad), Pin::new(a)]);
    problem.add_net(1, &[Pin::new(pad), Pin::new(b)]);
    let mut hot = CongestionGrid::new(problem.extent(), 1, 1.0);
    for x in 0..4 {
        hot.add_demand_at(Point::new(x, 0), 10.0);
    }
    assert!(hot.overflow(&Rect::new(Point::new(0, 0), Point::new(3, 0))) > 0);
    assert_eq!(
        hot.overflow(&Rect::new(Point::new(5, 0), Point::new(8, 0))),
        0
    );

    let plain = anneal(&problem, Wirelength, AnnealOptions::default()).expect("anneal");
    assert_eq!(plain.wirelength, 2);
    let cost = (
        Wirelength,
        CongestionCost {
            estimate: &hot,
            factor_permille: 1000,
        },
    );
    let aware = anneal(&problem, &cost, AnnealOptions::default()).expect("anneal");
    assert_legal(&problem, &aware.placement);
    for item in [a, b] {
        assert!(aware.placement.origin(item).x > 4, "{:?}", aware.placement);
    }
    assert_eq!(aware.wirelength, 3);
    assert_eq!(aware.cost, 3);
}

/// A random problem on sites: kinds by column, some items fixed, some vertical macros.
fn sites_problem(
    width: u32,
    height: u32,
    fill: u32,
    fixed: u32,
    macros: u32,
    seed: u64,
) -> PlacementProblem {
    let mut rng = crate::rng::Rng::new(seed);
    let kind_of = |x: u32| u32::from(x % 4 == 3);
    let mut sites = Vec::new();
    for x in 0..width {
        for y in 0..height {
            sites.push(Site::new(
                Point::new(i64::from(x), i64::from(y)),
                kind_of(x),
            ));
        }
    }
    let mut free: Vec<usize> = (0..sites.len()).collect();
    rng.shuffle(&mut free);
    let count = (sites.len() as u32 * fill / 100).max(2) as usize;
    let mut problem = PlacementProblem::new(Layout::Sites(sites.clone()));
    let mut items = Vec::new();
    for (k, &s) in free.iter().take(count).enumerate() {
        let mut item = Item::new().kind(sites[s].kind);
        if (k as u32) < fixed {
            item = item.fixed_at_site(SiteId(s as u32));
        }
        items.push(problem.add_item(item));
    }
    // Macros of two items of kind 0, one above the other, from movable items.
    let pairable: Vec<ItemId> = items
        .iter()
        .copied()
        .filter(|&i| problem.item(i).site_kind() == 0 && problem.item(i).is_movable())
        .collect();
    for pair in pairable.as_chunks::<2>().0.iter().take(macros as usize) {
        problem.add_macro(&[(pair[0], Point::new(0, 0)), (pair[1], Point::new(0, 1))]);
    }
    for _ in 0..count {
        let d = 2 + rng.below(3) as usize;
        let pins: Vec<Pin> = (0..d)
            .map(|_| Pin::new(items[rng.below(items.len() as u64) as usize]))
            .collect();
        problem.add_net(1 + rng.below(3) as u32, &pins);
    }
    problem
}

/// A random problem in free space: rectangles of various sizes over at most about a third of
/// the area, the first few fixed apart from each other, some able to turn.
fn free_problem(count: u32, fixed: u32, seed: u64) -> PlacementProblem {
    let mut rng = crate::rng::Rng::new(seed);
    let area = Rect::with_size(0, 0, 60, 60);
    let mut problem = PlacementProblem::new(Layout::FreeSpace { area, pitch: 1 });
    let mut items = Vec::new();
    for k in 0..count {
        let (w, h) = (2 + rng.below(7) as i64, 2 + rng.below(7) as i64);
        let mut item = Item::new().size(w, h);
        if k < fixed {
            item = item.fixed_at(Point::new(i64::from(k) * 15, 25), 0);
        } else if rng.below(2) == 0 {
            item = item.rotations(&[0, 90_000]);
        }
        items.push(problem.add_item(item));
    }
    for _ in 0..count {
        let d = 2 + rng.below(3) as usize;
        let pins: Vec<Pin> = (0..d)
            .map(|_| Pin::at(items[rng.below(items.len() as u64) as usize], 1, 1))
            .collect();
        problem.add_net(1, &pins);
    }
    problem
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn placements_on_sites_are_legal_and_deterministic(
        width in 2u32..9,
        height in 2u32..9,
        fill in 30u32..101,
        fixed in 0u32..4,
        macros in 0u32..3,
        seed: u64,
    ) {
        let problem = sites_problem(width, height, fill, fixed, macros, seed);
        prop_assume!(problem.validate().is_ok());
        let options = AnnealOptions { seed, inner_num: 0.3, ..AnnealOptions::default() };
        let first = anneal(&problem, Wirelength, options);
        prop_assert_eq!(&first, &anneal(&problem, Wirelength, options));
        if let Ok(outcome) = &first {
            prop_assert!(check(&problem, &outcome.placement).is_empty());
            prop_assert_eq!(outcome.wirelength, problem.wirelength(&outcome.placement));
        } else {
            // Only macros can make legalization fail where there are sites enough.
            prop_assert!(macros > 0);
        }
        let options = AnalyticOptions { seed, ..AnalyticOptions::default() };
        let placed = place_analytic(&problem, Wirelength, options);
        prop_assert_eq!(&placed, &place_analytic(&problem, Wirelength, options));
        if let Ok((outcome, _)) = &placed {
            prop_assert!(check(&problem, &outcome.placement).is_empty());
        } else {
            prop_assert!(macros > 0);
        }
    }

    #[test]
    fn placements_in_free_space_are_legal_and_deterministic(
        count in 2u32..30,
        fixed in 0u32..4,
        seed: u64,
    ) {
        let problem = free_problem(count, fixed, seed);
        let options = AnnealOptions { seed, inner_num: 0.3, ..AnnealOptions::default() };
        let first = anneal(&problem, Wirelength, options);
        prop_assert_eq!(&first, &anneal(&problem, Wirelength, options));
        let outcome = first.expect("a third of the area always fits");
        prop_assert!(check(&problem, &outcome.placement).is_empty());
        let options = AnalyticOptions { seed, ..AnalyticOptions::default() };
        let placed = place_analytic(&problem, Wirelength, options);
        prop_assert_eq!(&placed, &place_analytic(&problem, Wirelength, options));
        let (outcome, _) = placed.expect("a third of the area always fits");
        prop_assert!(check(&problem, &outcome.placement).is_empty());
    }

    #[test]
    fn legalize_is_legal_from_any_targets(
        width in 2u32..9,
        height in 2u32..9,
        fill in 30u32..101,
        fixed in 0u32..4,
        seed: u64,
        targets in proptest::collection::vec((-3i64..12, -3i64..12), 64),
    ) {
        let problem = sites_problem(width, height, fill, fixed, 0, seed);
        prop_assume!(problem.validate().is_ok());
        let n = problem.items().len();
        let points: Vec<Point> = (0..n).map(|i| {
            let (x, y) = targets[i % targets.len()];
            Point::new(x, y)
        }).collect();
        let placement = legalize(&problem, &points, &vec![0; n]).expect("enough sites");
        prop_assert!(check(&problem, &placement).is_empty());
    }
}
