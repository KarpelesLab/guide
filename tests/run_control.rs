//! Run control in every domain, through the public API: a run stopped by its work budget,
//! deadline or flag returns a legal result and says why; a work budget gives the same result
//! every time; and the default control gives what the entry point without one does.

#![allow(clippy::field_reassign_with_default)]

#[cfg(feature = "fpga")]
mod common;

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use guide::control::{Progress, RunControl, Stage, Stopped};
use guide::place::{
    AnalyticOptions, AnnealOptions, IncrementalOptions, PlaceStop, Prior, Wirelength, anneal,
    anneal_with, check, place_analytic, place_analytic_with, place_incremental,
    place_incremental_with, synth,
};

fn budget(work: u64) -> RunControl<'static> {
    let mut control = RunControl::default();
    control.work_budget = Some(work);
    control
}

/// A deadline that has passed already.
fn past() -> RunControl<'static> {
    let mut control = RunControl::default();
    control.deadline = Some(Instant::now());
    control
}

#[test]
fn annealing_stops_legally_at_a_budget_the_same_way_every_time() {
    let problem = &synth::grid_with_pads(10, 10, 4).problem;
    let plain = anneal(problem, Wirelength, AnnealOptions::default()).unwrap();
    let unlimited = anneal_with(
        problem,
        Wirelength,
        AnnealOptions::default(),
        &RunControl::default(),
    )
    .unwrap();
    assert_eq!(unlimited, plain, "no limit, no change");
    assert_eq!(plain.stop, PlaceStop::Converged);
    assert_eq!(plain.stop.stopped(), None);

    let work = plain.moves / 3;
    let stopped =
        anneal_with(problem, Wirelength, AnnealOptions::default(), &budget(work)).unwrap();
    assert_eq!(stopped.stop.stopped(), Some(Stopped::WorkBudget));
    assert_eq!(stopped.moves, work);
    assert!(check(problem, &stopped.placement).is_empty());
    let again = anneal_with(problem, Wirelength, AnnealOptions::default(), &budget(work)).unwrap();
    assert_eq!(again, stopped, "a budget is reproducible");
    // A budget the run doesn't reach changes nothing.
    let roomy = anneal_with(
        problem,
        Wirelength,
        AnnealOptions::default(),
        &budget(u64::MAX),
    )
    .unwrap();
    assert_eq!(roomy, plain);
}

#[test]
fn a_flag_or_a_deadline_stops_annealing_with_a_legal_placement() {
    let problem = &synth::grid_with_pads(8, 8, 2).problem;
    let set = AtomicBool::new(true);
    let mut control = RunControl::default();
    control.cancel = Some(&set);
    let cancelled = anneal_with(problem, Wirelength, AnnealOptions::default(), &control).unwrap();
    assert_eq!(cancelled.stop, PlaceStop::Cancelled);
    assert!(check(problem, &cancelled.placement).is_empty());

    let late = anneal_with(problem, Wirelength, AnnealOptions::default(), &past()).unwrap();
    assert_eq!(late.stop, PlaceStop::Deadline);
    assert!(check(problem, &late.placement).is_empty());

    // The options' own deadline does the same.
    let mut options = AnnealOptions::default();
    options.deadline = Some(Instant::now());
    let own = anneal(problem, Wirelength, options).unwrap();
    assert_eq!(own.stop.stopped(), Some(Stopped::Deadline));
}

#[test]
fn analytic_and_incremental_placement_stop_legally() {
    let problem = &synth::grid_with_pads(8, 8, 5).problem;
    let (plain, plain_report) =
        place_analytic(problem, Wirelength, AnalyticOptions::default()).unwrap();
    let (same, same_report) = place_analytic_with(
        problem,
        Wirelength,
        AnalyticOptions::default(),
        &RunControl::default(),
    )
    .unwrap();
    assert_eq!((same, same_report), (plain.clone(), plain_report));

    let (late, report) =
        place_analytic_with(problem, Wirelength, AnalyticOptions::default(), &past()).unwrap();
    assert_eq!(late.stop, PlaceStop::Deadline);
    assert_eq!(report.iterations, 0, "global placement stopped at once");
    assert!(check(problem, &late.placement).is_empty());

    let (spent, _) = place_analytic_with(
        problem,
        Wirelength,
        AnalyticOptions::default(),
        &budget(500),
    )
    .unwrap();
    assert_eq!(spent.stop.stopped(), Some(Stopped::WorkBudget));
    assert_eq!(spent.moves, 500);
    assert!(check(problem, &spent.placement).is_empty());

    let priors = Prior::all(&plain.placement);
    let options = IncrementalOptions::default();
    let base = place_incremental(problem, Wirelength, &priors, options).unwrap();
    let unlimited = place_incremental_with(
        problem,
        Wirelength,
        &priors,
        options,
        &RunControl::default(),
    )
    .unwrap();
    assert_eq!(unlimited.outcome, base.outcome);
    let cut = place_incremental_with(problem, Wirelength, &priors, options, &budget(100)).unwrap();
    assert_eq!(cut.outcome.stop.stopped(), Some(Stopped::WorkBudget));
    assert!(check(problem, &cut.outcome.placement).is_empty());
}

#[test]
fn progress_is_heard_and_a_flag_set_by_the_listener_stops_the_run() {
    let problem = &synth::grid_with_pads(10, 10, 6).problem;
    let stop = AtomicBool::new(false);
    let heard = std::cell::RefCell::new(Vec::new());
    let listen = |p: &Progress| {
        heard.borrow_mut().push((p.stage, p.step, p.work));
        if p.stage == Stage::Anneal && p.step >= 2 {
            stop.store(true, core::sync::atomic::Ordering::Relaxed);
        }
    };
    let mut control = RunControl::default();
    control.cancel = Some(&stop);
    control.progress = Some(&listen);
    control.progress_interval = std::time::Duration::ZERO;
    let (outcome, _) =
        place_analytic_with(problem, Wirelength, AnalyticOptions::default(), &control).unwrap();
    assert_eq!(outcome.stop, PlaceStop::Cancelled);
    assert!(check(problem, &outcome.placement).is_empty());
    let heard = heard.into_inner();
    assert!(heard.iter().any(|h| h.0 == Stage::Global), "{heard:?}");
    assert!(heard.iter().any(|h| h.0 == Stage::Anneal), "{heard:?}");
    // Work only grows.
    assert!(heard.windows(2).all(|w| w[0].2 <= w[1].2), "{heard:?}");
}

#[cfg(feature = "fpga")]
mod fpga {
    use super::*;
    use crate::common::island::island_device;
    use guide::fpga::{
        LoopOptions, NetOptions, PlaceDelayModel, PlacerOptions, ProblemOptions,
        build_place_problem, constant_nets, place_design, place_design_with, place_route_loop,
        place_route_loop_with, read_architecture, read_netlist, read_rr_graph,
    };

    const NET: &str = include_str!("../testdata/vtr/mult_4x4.net");
    const ARCH: &str = include_str!("../testdata/vtr/k6_N10_40nm.xml");

    #[test]
    fn placing_and_the_loop_stop_legally_and_reproducibly() {
        let device = read_rr_graph(island_device(4, 12).as_bytes()).unwrap();
        let netlist = read_netlist(NET.as_bytes()).unwrap();
        let arch = read_architecture(ARCH.as_bytes()).unwrap();
        let mut options = ProblemOptions::default();
        options.ignored_nets = constant_nets(&netlist);
        let problem = build_place_problem(&device, &netlist, &arch, &options).unwrap();
        let delays = PlaceDelayModel::from_rr_graph(&device);
        let placer = PlacerOptions::default();

        let plain = place_design(&problem, &delays, None, &placer).unwrap();
        let same =
            place_design_with(&problem, &delays, None, &placer, &RunControl::default()).unwrap();
        assert_eq!(same, plain);
        assert_eq!(plain.stopped, None);

        let work = plain.moves / 2;
        let cut = place_design_with(&problem, &delays, None, &placer, &budget(work)).unwrap();
        assert_eq!(cut.stopped, Some(Stopped::WorkBudget));
        assert_eq!(cut.moves, work);
        assert!(check(problem.problem(), &cut.placement).is_empty());
        let again = place_design_with(&problem, &delays, None, &placer, &budget(work)).unwrap();
        assert_eq!(again, cut);

        let late = place_design_with(&problem, &delays, None, &placer, &past()).unwrap();
        assert_eq!(late.stopped, Some(Stopped::Deadline));
        assert!(check(problem.problem(), &late.placement).is_empty());

        let mut nets = NetOptions::default();
        nets.constant_nets = constant_nets(&netlist);
        let engine = device.engine_options();
        let looped = |control: &RunControl<'_>| {
            place_route_loop_with(
                &device,
                &netlist,
                &problem,
                &delays,
                None,
                &nets,
                engine,
                plain.placement.clone(),
                &LoopOptions::default(),
                control,
            )
            .unwrap()
        };
        let whole = place_route_loop(
            &device,
            &netlist,
            &problem,
            &delays,
            None,
            &nets,
            engine,
            plain.placement.clone(),
            &LoopOptions::default(),
        )
        .unwrap();
        let unlimited = looped(&RunControl::default());
        assert_eq!(unlimited.placement, whole.placement);
        assert_eq!(unlimited.outcome.routes, whole.outcome.routes);
        assert_eq!(unlimited.stopped, None);
        let spent = whole.outcome.routes.stats.nodes_expanded / 2;
        let cut = looped(&budget(spent));
        assert_eq!(cut.stopped, Some(Stopped::WorkBudget));
        assert!(check(problem.problem(), &cut.placement).is_empty());
        assert_eq!(looped(&budget(spent)).outcome.routes, cut.outcome.routes);
        let late = looped(&past());
        assert_eq!(late.stopped, Some(Stopped::Deadline));
    }
}

#[cfg(feature = "mapf")]
mod mapf {
    use super::*;
    use guide::mapf::{Agent, Graph, Instance, SolveOptions, check, solve, solve_with};

    #[test]
    fn solving_stops_with_a_reason_and_a_budget_is_reproducible() {
        let open: Vec<bool> = (0..64)
            .map(|i| ![9, 14, 27, 36, 49, 54].contains(&i))
            .collect();
        let graph = Graph::from_grid(8, 8, &open).unwrap();
        let n = graph.vertex_count();
        let agents: Vec<Agent> = (0..6)
            .map(|i| Agent {
                start: i,
                goal: n - 1 - i,
            })
            .collect();
        let instance = Instance::new(&graph, agents.clone()).unwrap();
        let options = SolveOptions::default();
        let plain = solve(&instance, &options);
        assert_eq!(
            solve_with(&instance, &options, &RunControl::default()),
            plain
        );
        assert_eq!(plain.stopped, None);

        let spent = plain.nodes_expanded() / 2;
        let cut = solve_with(&instance, &options, &budget(spent));
        assert_eq!(cut.stopped, Some(Stopped::WorkBudget));
        assert_eq!(solve_with(&instance, &options, &budget(spent)), cut);
        if let Some(plans) = &cut.plans {
            assert!(check::check(&graph, &agents, plans).is_ok());
        }

        let late = solve_with(&instance, &options, &past());
        assert_eq!(late.stopped, Some(Stopped::Deadline));
        if let Some(plans) = &late.plans {
            assert!(check::check(&graph, &agents, plans).is_ok());
        }
    }
}

#[cfg(feature = "asic")]
mod asic {
    use super::*;
    use guide::asic::{AsicOptions, read_design, route_design, route_design_with};

    const DESIGN: &str = "grid 3 3 2
vertical capacity 0 2
horizontal capacity 2 0
minimum width 1 1
minimum spacing 1 1
via spacing 1 1
0 0 10 10
num net 2
A 0 2 1
 5  5 1
25 25 1
B 1 2 1
 5 25 1
25  5 1
";

    #[test]
    fn routing_stops_with_every_net_routed_and_says_why() {
        let design = read_design(DESIGN.as_bytes()).unwrap();
        let options = AsicOptions::default();
        let plain = route_design(&design, &options);
        let same = route_design_with(&design, &options, &RunControl::default());
        assert_eq!(same.routes, plain.routes);
        assert_eq!(same.stopped, None);

        // With no work at all, the pattern routes are what is left, and given layers.
        let cut = route_design_with(&design, &options, &budget(0));
        assert_eq!(cut.stopped, Some(Stopped::WorkBudget));
        assert!(cut.routes.iter().all(|r| !r.is_empty()));
        assert_eq!(
            route_design_with(&design, &options, &budget(0)).routes,
            cut.routes
        );

        let late = route_design_with(&design, &options, &past());
        assert_eq!(late.stopped, Some(Stopped::Deadline));
        assert!(late.routes.iter().all(|r| !r.is_empty()));
    }
}

#[cfg(feature = "kicad")]
mod kicad {
    use super::*;
    use guide::kicad::{FlowOptions, place_and_route_with, placement_design, read};
    use guide::pcb::place::{
        PlaceOptions, outline_overlaps, place_components, place_components_with,
    };

    fn board() -> (String, String) {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/kicad/dotbots/");
        let pcb = std::fs::read_to_string(format!("{dir}TS4231-BOB.kicad_pcb")).unwrap();
        let pro = std::fs::read_to_string(format!("{dir}TS4231-BOB.kicad_pro")).unwrap();
        (pcb, pro)
    }

    #[test]
    fn board_placement_and_the_flow_stop_with_a_legal_placement() {
        let (pcb, pro) = board();
        let board = read(&pcb, Some(&pro)).unwrap();
        let design = placement_design(&board);
        let options = PlaceOptions::default();
        let plain = place_components(&design, &options).unwrap();
        let same = place_components_with(&design, &options, &RunControl::default()).unwrap();
        assert_eq!(same, plain);
        let legal = |placed: &[guide::pcb::place::Placed]| {
            outline_overlaps(&design, placed)
                .into_iter()
                .all(|(a, b)| design.components[a].fixed && design.components[b].fixed)
        };
        for control in [budget(200), past()] {
            let cut = place_components_with(&design, &options, &control).unwrap();
            assert!(cut.stopped.is_some());
            assert!(legal(&cut.placed));
        }
        assert_eq!(
            place_components_with(&design, &options, &budget(200)).unwrap(),
            place_components_with(&design, &options, &budget(200)).unwrap()
        );

        // The whole flow, stopped in its first routing: the same board every time.
        let mut flow = FlowOptions::default();
        flow.rounds = 1;
        let work = 2_000;
        let first = place_and_route_with(&pcb, Some(&pro), &flow, &budget(work)).unwrap();
        assert_eq!(first.stopped, Some(Stopped::WorkBudget));
        assert!(legal(&first.placed));
        let again = place_and_route_with(&pcb, Some(&pro), &flow, &budget(work)).unwrap();
        assert_eq!(again.text, first.text);
        let late = place_and_route_with(&pcb, Some(&pro), &flow, &past()).unwrap();
        assert_eq!(late.stopped, Some(Stopped::Deadline));
        assert!(legal(&late.placed));
    }
}
