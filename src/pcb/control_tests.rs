//! Tests of run control, of what a run reports of the connections it leaves open, and of
//! the options that came with them.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::tests::{MM, design, outline, route_checked, smd};
use super::{
    Blocker, Cause, CopperKind, Design, DesignItem, Phase, Progress, RouteError, RouteOptions,
    Routed, RunControl, Stopped, route, route_with, unconnected,
};
use crate::geom::drc::{self, Keepout, Pad, Track};
use crate::geom::{LayerId, LayerSet, Point, Rect, Shape};
use crate::id::NetId;

/// Six nets across and six down on two layers, which all cross.
fn weave() -> Design {
    let mut pads = Vec::new();
    for k in 0..6 {
        let at = (2 * k + 2) * MM;
        pads.push(smd(0, at, 1 + k as u32));
        pads.push(smd(14 * MM, at, 1 + k as u32));
        pads.push(smd(at - MM, 0, 7 + k as u32));
        pads.push(smd(at - MM, 14 * MM, 7 + k as u32));
    }
    let mut d = design(2, pads, 13);
    d.outline = outline(Rect::new(
        Point::new(-MM, -MM),
        Point::new(15 * MM, 15 * MM),
    ));
    d
}

/// Five pads of one net in a row on one layer, the last walled in by four keepouts.
fn walled_in() -> Design {
    let pads: Vec<Pad> = (0..5).map(|k| smd(k * 2 * MM, 0, 1)).collect();
    let mut d = design(1, pads, 2);
    let x = 8 * MM;
    let wall = |a: Point, b: Point| Keepout {
        shape: Shape::rect(Rect::new(a, b)).unwrap(),
        layers: LayerSet::first(1),
        tracks: true,
        vias: true,
        pads: false,
        except: Vec::new(),
    };
    d.board.keepouts.extend([
        wall(Point::new(x - MM, -MM), Point::new(x + MM, -MM / 2)),
        wall(Point::new(x - MM, MM / 2), Point::new(x + MM, MM)),
        wall(Point::new(x - MM, -MM), Point::new(x - MM / 2, MM)),
        wall(Point::new(x + MM / 2, -MM), Point::new(x + MM, MM)),
    ]);
    d.outline = outline(Rect::new(
        Point::new(-2 * MM, -3 * MM),
        Point::new(11 * MM, 3 * MM),
    ));
    d
}

/// What must hold of any result, stopped or not: no rule broken, the nets in pieces are
/// the nets reported, and the open connections are the pieces less one.
fn check(design: &Design, routed: &Routed) {
    let board = routed.board(design);
    let found = drc::check(&board, &design.rules).unwrap();
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(unconnected(&board, &design.rules), routed.unrouted);
    let (needed, made) = routed.stats.islands;
    assert_eq!(routed.open.len(), needed - made);
    let mut nets: Vec<NetId> = routed.open.iter().map(|o| o.net).collect();
    nets.dedup();
    assert_eq!(nets, routed.unrouted);
    assert_eq!(routed.track_kinds.len(), routed.tracks.len());
    assert_eq!(routed.via_kinds.len(), routed.vias.len());
}

/// Everything of a result that a caller can see.
fn same(a: &Routed, b: &Routed) {
    assert_eq!(a.tracks, b.tracks);
    assert_eq!(a.vias, b.vias);
    assert_eq!(a.unrouted, b.unrouted);
    assert_eq!(a.stats, b.stats);
    assert_eq!(a.stopped, b.stopped);
    assert_eq!(a.open, b.open);
    assert_eq!(a.track_kinds, b.track_kinds);
}

#[test]
fn a_run_under_control_that_nothing_stops_is_the_run_without() {
    let d = weave();
    let plain = route_checked(&d, &RouteOptions::default());
    assert!(plain.unrouted.is_empty() && plain.stopped.is_none() && plain.open.is_empty());
    assert_eq!(plain.stats.islands, (12, 12));
    assert!(plain.stats.work >= plain.stats.expanded && plain.stats.work > 0);
    assert!(plain.track_kinds.iter().all(|k| *k == CopperKind::Plain));
    check(&d, &plain);

    let cancel = AtomicBool::new(false);
    let heard = |_: &Progress| {};
    let control = RunControl {
        cancel: Some(&cancel),
        deadline: Some(Instant::now() + Duration::from_secs(3600)),
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    same(
        &plain,
        &route_with(&d, &RouteOptions::default(), &control).unwrap(),
    );

    // A budget that is not spent changes nothing either.
    let options = RouteOptions {
        work_budget: Some(plain.stats.work + 1),
        ..RouteOptions::default()
    };
    same(&plain, &route(&d, &options).unwrap());
}

#[test]
fn a_work_budget_stops_a_run_the_same_way_every_time_and_on_any_thread() {
    let d = weave();
    let whole = route(&d, &RouteOptions::default()).unwrap();
    let mut options = RouteOptions::default();
    let mut seen_partial = false;
    for share in [0, 1, 2, 3, 5, 7] {
        let budget = whole.stats.work * share / 8;
        options.work_budget = Some(budget);
        let first = route(&d, &options).unwrap();
        check(&d, &first);
        assert_eq!(first.stopped, Some(Stopped::WorkBudget), "budget {budget}");
        assert!(first.stats.work <= budget);
        // The last of the work is cleanup's: a budget that ends there leaves nothing open.
        seen_partial |= !first.tracks.is_empty() && !first.open.is_empty();
        // Again here, and on four threads at once, which is as many thread counts as a
        // router with no threads of its own can be run under.
        same(&first, &route(&d, &options).unwrap());
        std::thread::scope(|scope| {
            let runs: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| route(&d, &options).unwrap()))
                .collect();
            for run in runs {
                same(&first, &run.join().unwrap());
            }
        });
    }
    assert!(seen_partial, "no budget left a part of the board routed");
    // Nothing spent, nothing routed, and every connection is reported as stopped.
    options.work_budget = Some(0);
    let none = route(&d, &options).unwrap();
    assert!(none.tracks.is_empty() && none.vias.is_empty());
    assert_eq!(none.open.len(), 12);
    assert!(none.open.iter().all(|o| o.cause == Cause::Stopped));
    assert_eq!(none.stats.work, 0);
}

#[test]
fn a_cancel_flag_stops_a_run_with_what_it_has_routed() {
    let d = weave();
    let cancel = AtomicBool::new(false);
    // The flag is set from the callback, once three nets are routed, so that the test
    // doesn't depend on the clock.
    let heard = |p: &Progress| {
        if p.nets_routed >= 3 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let control = RunControl {
        cancel: Some(&cancel),
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, Some(Stopped::Cancelled));
    assert_eq!(routed.stats.routed, 3);
    assert_eq!(routed.unrouted.len(), 9);
    assert_eq!(routed.open.len(), 9);
    assert!(routed.open.iter().all(|o| o.cause == Cause::Stopped));
    assert!(!routed.tracks.is_empty());
}

#[test]
fn a_deadline_stops_a_run() {
    let d = weave();
    let control = RunControl {
        deadline: Some(Instant::now()),
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    check(&d, &routed);
    assert_eq!(routed.stopped, Some(Stopped::Deadline));
    assert!(routed.tracks.is_empty());
    assert_eq!(routed.open.len(), 12);
}

#[test]
fn progress_tells_of_each_phase_and_no_more_often_than_asked() {
    let d = weave();
    let log: RefCell<Vec<Progress>> = RefCell::new(Vec::new());
    let heard = |p: &Progress| log.borrow_mut().push(*p);
    let mut control = RunControl {
        progress: Some(&heard),
        progress_interval: Duration::ZERO,
        ..RunControl::default()
    };
    let routed = route_with(&d, &RouteOptions::default(), &control).unwrap();
    let all = log.take();
    let mut phases: Vec<Phase> = all.iter().map(|p| p.phase).collect();
    phases.dedup();
    assert_eq!(
        phases,
        [
            Phase::Prepare,
            Phase::Global,
            Phase::Detail,
            Phase::Cleanup,
            Phase::Report
        ]
    );
    assert!(all.windows(2).all(|w| w[0].work <= w[1].work));
    let last = all.last().unwrap();
    assert_eq!((last.nets, last.nets_routed), (12, 12));
    assert_eq!((last.connections, last.connected), (12, 12));
    assert_eq!(last.work, routed.stats.work);
    // Every net is heard of as it is taken from the queue.
    assert!(all.iter().filter(|p| p.phase == Phase::Detail).count() >= 12);

    // With an interval the run never reaches, only the changes of phase are told.
    control.progress_interval = Duration::from_secs(3600);
    route_with(&d, &RouteOptions::default(), &control).unwrap();
    assert_eq!(log.borrow().len(), phases.len());
}

#[test]
fn a_net_that_cannot_be_routed_whole_keeps_its_tree_if_asked() {
    let d = walled_in();
    let lost = route_checked(&d, &RouteOptions::default());
    let options = RouteOptions {
        keep_partial: true,
        ..RouteOptions::default()
    };
    let kept = route_checked(&d, &options);
    check(&d, &kept);
    assert_eq!(kept.unrouted, [NetId(1)]);
    assert_eq!((lost.stats.islands, kept.stats.islands), ((4, 0), (4, 3)));
    assert_eq!((kept.stats.connections, kept.stats.connected), (4, 3));
    assert!(lost.tracks.is_empty() && !kept.tracks.is_empty());
    // One connection is open: to the pad walled in, from its neighbour.
    assert_eq!(kept.open.len(), 1);
    let open = &kept.open[0];
    assert_eq!(
        (open.net, open.from.pad, open.to.pad),
        (NetId(1), Some(3), Some(4))
    );
    assert_eq!(open.cause, lost.open[3].cause);
}

#[test]
fn open_connections_say_why() {
    // A pad walled in: as the net is routed today it fails whole, and four connections
    // are open for the one pad, in the order of the pads they end at, each from the pad
    // before. The last is to the pad shut in, by the four keepouts.
    let d = walled_in();
    let routed = route_checked(&d, &RouteOptions::default());
    check(&d, &routed);
    assert_eq!(routed.unrouted, [NetId(1)]);
    assert!(routed.tracks.is_empty());
    assert_eq!(routed.stats.islands, (4, 0));
    let ends: Vec<_> = routed.open.iter().map(|o| (o.from.pad, o.to.pad)).collect();
    let pairs: Vec<_> = (0..4).map(|k| (Some(k), Some(k + 1))).collect();
    assert_eq!(ends, pairs);
    let last = &routed.open[3];
    assert_eq!(last.to.at, Point::new(8 * MM, 0));
    assert_eq!(last.to.layers, LayerSet::first(1));
    let walls: Vec<Blocker> = (0..4)
        .map(|k| Blocker::Item(DesignItem::Keepout(k)))
        .collect();
    assert_eq!(last.cause, Cause::Enclosed { by: walls });
    // The others are not shut in, and the search for the net found no path.
    assert!(routed.open[..3].iter().all(|o| o.cause == Cause::NoPath));

    // No via, and pads on different layers.
    let mut d = design(2, vec![smd(0, 0, 1), smd(4 * MM, 0, 1)], 2);
    d.board.pads[1].layers = LayerSet::EMPTY.with(LayerId(1));
    d.nets[1].via = None;
    let routed = route_checked(&d, &RouteOptions::default());
    check(&d, &routed);
    assert_eq!(routed.open.len(), 1);
    assert_eq!(routed.open[0].cause, Cause::NoLayer);

    // A search that runs out of nodes.
    let d = design(1, vec![smd(0, 0, 1), smd(30 * MM, 0, 1)], 2);
    let options = RouteOptions {
        max_expansions: 20,
        ..RouteOptions::default()
    };
    let routed = route_checked(&d, &options);
    check(&d, &routed);
    assert_eq!(routed.open[0].cause, Cause::OutOfBudget);
    assert_eq!(routed.stopped, None);

    // Another net's route in the way: on one layer, one of two nets that cross is shut
    // off by the other's track, from edge to edge.
    let mut d = design(
        1,
        vec![
            smd(0, 5 * MM, 1),
            smd(10 * MM, 5 * MM, 1),
            smd(5 * MM, 0, 2),
            smd(5 * MM, 10 * MM, 2),
        ],
        3,
    );
    d.outline = outline(Rect::new(
        Point::new(-600_000, -600_000),
        Point::new(10_600_000, 10_600_000),
    ));
    let routed = route_checked(&d, &RouteOptions::default());
    check(&d, &routed);
    assert_eq!(routed.open.len(), 1);
    let open = &routed.open[0];
    let other = NetId(3 - open.net.0);
    assert!(routed.tracks.iter().all(|t| t.net == Some(other)));
    assert_eq!(open.cause, Cause::NoPath);
}

#[test]
fn an_error_names_the_item_at_fault() {
    let mut d = weave();
    let track = |width| Track {
        start: Point::new(0, 0),
        end: Point::new(MM, 0),
        mid: None,
        width,
        layer: LayerId(0),
        net: None,
    };
    d.board.tracks = vec![track(100_000), track(0)];
    let error = route(&d, &RouteOptions::default()).unwrap_err();
    assert!(
        matches!(
            error,
            RouteError::Item {
                item: DesignItem::Track(1),
                ..
            }
        ),
        "{error:?}"
    );
    assert!(error.to_string().contains("track 1"), "{error}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn a_seed_gives_another_order_and_the_same_one_every_time() {
    // On one layer, one of two nets that cross is routed: which depends on the order.
    let mut d = design(
        1,
        vec![
            smd(0, 5 * MM, 1),
            smd(10 * MM, 5 * MM, 1),
            smd(5 * MM, 0, 2),
            smd(5 * MM, 10 * MM, 2),
        ],
        3,
    );
    d.outline = outline(Rect::new(
        Point::new(-600_000, -600_000),
        Point::new(10_600_000, 10_600_000),
    ));
    let mut options = RouteOptions {
        rip_ups: 0,
        ..RouteOptions::default()
    };
    let plain = route_checked(&d, &options);
    let mut others = 0;
    for seed in 0..16 {
        options.seed = Some(seed);
        let seeded = route_checked(&d, &options);
        check(&d, &seeded);
        same(&seeded, &route(&d, &options).unwrap());
        assert_eq!(seeded.unrouted.len(), 1);
        others += usize::from(seeded.unrouted != plain.unrouted);
    }
    assert!(
        others > 0 && others < 16,
        "{others} of 16 seeds changed the order"
    );
}

#[test]
fn a_reroute_tags_what_it_kept_and_stops_as_a_route_does() {
    use super::eco::{self, Change};
    let d = weave();
    let before = route(&d, &RouteOptions::default()).unwrap();
    let change = Change::new(vec![NetId(1)], Vec::new());
    let after = eco::reroute(
        &d,
        &before.tracks,
        &before.vias,
        &change,
        &RouteOptions::default(),
    )
    .unwrap();
    assert!(after.routed.unrouted.is_empty());
    let r = &after.routed;
    assert_eq!(r.track_kinds.len(), r.tracks.len());
    for (t, k) in r.tracks.iter().zip(&r.track_kinds) {
        let rerouted = t.net == Some(NetId(1));
        assert_eq!(*k == CopperKind::Plain, rerouted, "{t:?} {k:?}");
        assert_eq!(*k == CopperKind::Kept, !rerouted);
    }
    // Stopped at once, it keeps what it kept and leaves the changed net open.
    let cancel = AtomicBool::new(true);
    let calls = Cell::new(0);
    let heard = |_: &Progress| calls.set(calls.get() + 1);
    let control = RunControl {
        cancel: Some(&cancel),
        progress: Some(&heard),
        ..RunControl::default()
    };
    let stopped = eco::reroute_with(
        &d,
        &before.tracks,
        &before.vias,
        &change,
        &RouteOptions::default(),
        &control,
    )
    .unwrap();
    assert_eq!(stopped.routed.stopped, Some(Stopped::Cancelled));
    assert_eq!(stopped.routed.unrouted, [NetId(1)]);
    assert_eq!(stopped.routed.open.len(), 1);
    assert!(
        stopped
            .routed
            .track_kinds
            .iter()
            .all(|k| *k == CopperKind::Kept)
    );
    assert!(calls.get() > 0);
    assert!(
        drc::check(&stopped.routed.board(&d), &d.rules)
            .unwrap()
            .is_empty()
    );
}

/// Nets across and down on one layer and a second layer too narrow for all of them: they
/// are in each other's way, and rip-up takes them out again and again.
fn crowded() -> Design {
    let mut d = weave();
    // The second layer only in a band through the middle.
    let band = |a: Point, b: Point| Keepout {
        shape: Shape::rect(Rect::new(a, b)).unwrap(),
        layers: LayerSet::EMPTY.with(LayerId(1)),
        tracks: true,
        vias: true,
        pads: false,
        except: Vec::new(),
    };
    d.board.keepouts.extend([
        band(Point::new(-MM, -MM), Point::new(15 * MM, 5 * MM)),
        band(Point::new(-MM, 9 * MM), Point::new(15 * MM, 15 * MM)),
    ]);
    d
}

#[test]
fn a_stopped_run_keeps_the_most_connections_it_had() {
    let d = crowded();
    let whole = route(&d, &RouteOptions::default()).unwrap();
    assert!(whole.stats.rip_ups > 0, "nothing was ripped up");
    let mut options = RouteOptions::default();
    let mut most = 0;
    for step in 0..=64 {
        options.work_budget = Some(whole.stats.work * step / 64);
        let r = route(&d, &options).unwrap();
        check(&d, &r);
        // More work never leaves fewer connections: the nets that rip-up had out when
        // the budget ran out are as they were before it took them out.
        assert!(
            r.stats.connected >= most,
            "{} connections with {:?}, {most} with less",
            r.stats.connected,
            options.work_budget
        );
        most = r.stats.connected;
    }
}

#[test]
fn a_run_stopped_by_its_flag_fills_its_zones_unless_told_not_to() {
    let d = super::zone_tests::plane_board();
    let cancel = AtomicBool::new(true);
    let control = RunControl {
        cancel: Some(&cancel),
        ..RunControl::default()
    };
    let filled = route_with(&d, &RouteOptions::default(), &control).unwrap();
    assert_eq!(filled.stopped, Some(Stopped::Cancelled));
    assert!(!filled.fills.is_empty());

    let control = RunControl {
        fill_on_stop: false,
        ..control
    };
    let bare = route_with(&d, &RouteOptions::default(), &control).unwrap();
    check(&d, &bare);
    assert_eq!(bare.stopped, Some(Stopped::Cancelled));
    assert!(bare.fills.is_empty());
    // A run that is not stopped fills them, whatever it was told, and so does one that
    // its work budget stops.
    cancel.store(false, Ordering::Relaxed);
    let whole = route_with(&d, &RouteOptions::default(), &control).unwrap();
    assert!(whole.stopped.is_none() && !whole.fills.is_empty());
    let options = RouteOptions {
        work_budget: Some(0),
        ..RouteOptions::default()
    };
    let spent = route_with(&d, &options, &control).unwrap();
    assert_eq!(spent.stopped, Some(Stopped::WorkBudget));
    assert!(!spent.fills.is_empty());
}
