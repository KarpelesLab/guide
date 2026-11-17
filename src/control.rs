//! Run control: one way to stop a long run and to hear how it goes, in every domain.
//!
//! A [`RunControl`] is what a caller hands to the entry points that take one, the functions
//! named `…_with`: [`place::anneal_with`](crate::place::anneal_with),
//! [`place::place_analytic_with`](crate::place::place_analytic_with) and
//! [`place::place_incremental_with`](crate::place::place_incremental_with), and with their
//! features `fpga::place_design_with`, `fpga::refine_placement_with`,
//! `fpga::place_route_loop_with`, `pcb::place::place_components_with`,
//! `pcb::place::refine_components_with`, `kicad::place_and_route_with`, `asic::route_design_with` and `mapf::solve_with`. PCB routing
//! has a control of its own, `pcb::RunControl`, with a progress that knows its phases and nets,
//! and the [engine](crate::engine) and the [annealer](crate::place::Annealer) take a cancel flag
//! and a budget in their options, and are stepped by their callers.
//!
//! A control can stop a run three ways, and a stopped run says which, as a [`Stopped`]:
//!
//! - **A work budget**, [`RunControl::work_budget`], counted in the run's own units of work:
//!   annealing moves for placement, nodes expanded for routing by the engine and for PCB
//!   routing, and both together for the flows that place and route. A run stopped by its
//!   budget is the same for the same budget, on any machine and at any number of threads:
//!   this is the way to bound a run reproducibly.
//! - **A deadline**, [`RunControl::deadline`], by the clock. How far a run gets by then depends
//!   on the machine and its load, so the result does too.
//! - **A cancel flag**, [`RunControl::cancel`], set from another thread or from the progress
//!   callback. The result depends on when it was set.
//!
//! A run that is stopped returns its best legal result so far: a placement that is legal, the
//! best routes negotiation had, with what is left over said as it is said for a run that ends
//! on its own. Nothing here changes the result of a run that is left to finish: a control
//! with none of the three set, [`RunControl::default`], gives the same result, bit for bit, as
//! the entry point without `_with`, and a progress callback only listens.
//!
//! Engines take a single cancel flag, so a run with a deadline that routes with the engine
//! has a thread of its own watch the clock and the caller's flag, and set the engine's flag
//! once either says so. Runs on one thread, such as annealing, look at the clock themselves,
//! every thousand moves or so.
//!
//! ```
//! use std::sync::atomic::AtomicBool;
//! use std::time::{Duration, Instant};
//! use guide::control::{Progress, RunControl, Stopped};
//! use guide::place::{AnnealOptions, Wirelength, anneal_with, check, synth};
//!
//! let generated = synth::grid_with_pads(8, 8, 3);
//! let cancel = AtomicBool::new(false);
//! let heard = |p: &Progress| eprintln!("{:?} {}: {} moves", p.stage, p.step, p.work);
//! let mut control = RunControl::default();
//! control.cancel = Some(&cancel);
//! control.deadline = Some(Instant::now() + Duration::from_secs(60));
//! control.work_budget = Some(1000);
//! control.progress = Some(&heard);
//!
//! let outcome = anneal_with(&generated.problem, Wirelength, AnnealOptions::default(), &control)?;
//! assert!(check(&generated.problem, &outcome.placement).is_empty());
//! assert_eq!(outcome.stop.stopped(), Some(Stopped::WorkBudget));
//! assert_eq!(outcome.moves, 1000);
//! # Ok::<(), guide::place::PlaceError>(())
//! ```

use core::cell::Cell;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// Why a run stopped before it was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stopped {
    /// The work budget was spent: [`RunControl::work_budget`], or PCB routing's
    /// `RouteOptions::work_budget`. The result is the same for the same budget, on any
    /// machine.
    WorkBudget,
    /// The deadline passed. The result depends on when.
    Deadline,
    /// The cancel flag was set. The result depends on when.
    Cancelled,
}

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stopped::WorkBudget => "the work budget was spent",
            Stopped::Deadline => "the deadline passed",
            Stopped::Cancelled => "the run was cancelled",
        })
    }
}

/// What a run is doing, for [`Progress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stage {
    /// Global placement: analytic placement's solves and spreading.
    Global,
    /// Annealing, a temperature at a time.
    Anneal,
    /// Routing: negotiation by the engine, an iteration at a time, or PCB routing.
    Route,
    /// Routing again what a change touched, or a refinement after routing.
    Reroute,
    /// Layer assignment, after routing on a plane.
    Layers,
    /// Shortening a legal solution, as multi-agent path finding does.
    Improve,
}

/// Where a run is, as [`RunControl::progress`] hears it.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Progress {
    /// What the run is doing.
    pub stage: Stage,
    /// The round of a run that goes round, such as a loop of placement and routing, from 0
    /// for the first; 0 for a run that doesn't.
    pub round: u32,
    /// How far the stage is: the temperature of annealing, the iteration of negotiation or
    /// global placement, from 1, or 0 when the stage has just begun.
    pub step: u64,
    /// The work spent so far by the whole run, in the units of
    /// [`work_budget`](RunControl::work_budget).
    pub work: u64,
    /// The stage's cost so far, if it has one: the placement's cost while annealing, its
    /// wirelength in global placement, the routes' total cost in negotiation.
    pub cost: Option<f64>,
    /// What is still in conflict, if the stage counts it: resources overused in negotiation,
    /// connections open in PCB routing.
    pub conflicts: Option<u64>,
}

impl Progress {
    /// A report of `stage` at `step`, with nothing else known.
    pub fn new(stage: Stage, step: u64) -> Progress {
        Progress {
            stage,
            round: 0,
            step,
            work: 0,
            cost: None,
            conflicts: None,
        }
    }
}

/// What may stop a run from outside, how much work it may do, and who hears of its progress.
///
/// Create this with [`Default`] and set the fields; the default stops nothing and tells no
/// one, and a run with it is the same as one without. See the [module documentation](self).
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct RunControl<'a> {
    /// A flag that stops the run once set, from another thread or from the progress
    /// callback. The default is none.
    pub cancel: Option<&'a AtomicBool>,
    /// When to stop, by the clock. The default is none. A result stopped by it depends on the
    /// machine and its load.
    pub deadline: Option<Instant>,
    /// How much work the run may do, in its own units: see the [module
    /// documentation](self). The default is none, no limit. Where options have a budget of
    /// their own as well, such as [`AnnealOptions::max_moves`](crate::place::AnnealOptions)
    /// or [`EngineOptions::max_expansions`](crate::engine::EngineOptions), the run stops at
    /// whichever runs out first.
    pub work_budget: Option<u64>,
    /// Called as the run goes, on the thread that called the entry point: at every change of
    /// stage and step, no more often than
    /// [`progress_interval`](RunControl::progress_interval) unless the stage changes. The
    /// default is none.
    pub progress: Option<&'a dyn Fn(&Progress)>,
    /// The shortest time between two calls of [`progress`](RunControl::progress) within a
    /// stage. The default is 100 ms.
    pub progress_interval: Duration,
}

impl Default for RunControl<'_> {
    fn default() -> Self {
        RunControl {
            cancel: None,
            deadline: None,
            work_budget: None,
            progress: None,
            progress_interval: Duration::from_millis(100),
        }
    }
}

impl fmt::Debug for RunControl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunControl")
            .field("cancel", &self.cancel)
            .field("deadline", &self.deadline)
            .field("work_budget", &self.work_budget)
            .field("progress", &self.progress.map(|_| "Fn"))
            .field("progress_interval", &self.progress_interval)
            .finish()
    }
}

/// How long the thread that watches a deadline sleeps at most between looks at the caller's
/// flag.
#[cfg(any(feature = "fpga", feature = "mapf", feature = "asic"))]
const WATCH_INTERVAL: Duration = Duration::from_millis(5);

const NOT_STOPPED: u8 = 0;

fn code(why: Stopped) -> u8 {
    match why {
        Stopped::WorkBudget => 1,
        Stopped::Deadline => 2,
        Stopped::Cancelled => 3,
    }
}

fn from_code(code: u8) -> Option<Stopped> {
    match code {
        1 => Some(Stopped::WorkBudget),
        2 => Some(Stopped::Deadline),
        3 => Some(Stopped::Cancelled),
        _ => None,
    }
}

/// Sets `why` to `reason` if nothing stopped the run yet, and `stop` in any case.
fn halt_with(stop: &AtomicBool, why: &AtomicU8, reason: Stopped) {
    let _ = why.compare_exchange(
        NOT_STOPPED,
        code(reason),
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    stop.store(true, Ordering::Release);
}

/// One run's side of a [`RunControl`]: its work so far, why it stopped, and when it last told
/// of progress. Shared by every part of a run, so that a budget covers all of it.
pub(crate) struct Run<'a> {
    control: RunControl<'a>,
    /// Set once the run has to stop: what engines are given as their cancel flag.
    stop: AtomicBool,
    /// Why, as a [`code`]; the first reason wins.
    why: AtomicU8,
    work: Cell<u64>,
    round: Cell<u32>,
    last: Cell<Option<(Stage, Instant)>>,
}

impl<'a> Run<'a> {
    pub(crate) fn new(control: &RunControl<'a>) -> Run<'a> {
        let run = Run {
            control: *control,
            stop: AtomicBool::new(false),
            why: AtomicU8::new(NOT_STOPPED),
            work: Cell::new(0),
            round: Cell::new(0),
            last: Cell::new(None),
        };
        if control.work_budget == Some(0) {
            run.halt(Stopped::WorkBudget);
        }
        run
    }

    /// Stops the run, for `why` unless something stopped it already.
    pub(crate) fn halt(&self, why: Stopped) {
        halt_with(&self.stop, &self.why, why);
    }

    /// Why the run has stopped, if it has, as far as anything has looked: see
    /// [`poll`](Run::poll).
    pub(crate) fn stopped(&self) -> Option<Stopped> {
        from_code(self.why.load(Ordering::Acquire))
    }

    /// Looks at the caller's flag and the clock, and returns why the run has to stop, if it
    /// has to.
    pub(crate) fn poll(&self) -> Option<Stopped> {
        if self.stopped().is_none() {
            if self
                .control
                .cancel
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                self.halt(Stopped::Cancelled);
            } else if self.control.deadline.is_some_and(|d| Instant::now() >= d) {
                self.halt(Stopped::Deadline);
            }
        }
        self.stopped()
    }

    /// The caller's flag, for what reads it itself, as the annealer does.
    pub(crate) fn cancel(&self) -> Option<&'a AtomicBool> {
        self.control.cancel
    }

    /// The deadline, for what reads the clock itself, as the annealer does.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.control.deadline
    }

    /// The flag that is set once the run has to stop, for whatever reason.
    pub(crate) fn flag(&self) -> &AtomicBool {
        &self.stop
    }

    /// The work that the budget leaves, if there is a budget.
    pub(crate) fn work_left(&self) -> Option<u64> {
        self.control
            .work_budget
            .map(|b| b.saturating_sub(self.work.get()))
    }

    /// `own` budget, or less if the run's budget leaves less.
    pub(crate) fn budget(&self, own: u64) -> u64 {
        self.work_left().map_or(own, |left| own.min(left))
    }

    /// Charges work to the budget, which stops the run when it is spent.
    pub(crate) fn charge(&self, work: u64) {
        self.work.set(self.work.get().saturating_add(work));
        if self.work_left() == Some(0) {
            self.halt(Stopped::WorkBudget);
        }
    }

    /// Whether anyone listens to progress.
    pub(crate) fn listening(&self) -> bool {
        self.control.progress.is_some()
    }

    /// Tells of progress, with the run's round, and the work so far plus `pending`, work done
    /// and not yet charged: always at a change of stage, and otherwise no more often than the
    /// control's interval.
    pub(crate) fn tell(&self, mut progress: Progress, pending: u64) {
        let Some(callback) = self.control.progress else {
            return;
        };
        let now = Instant::now();
        let due = match self.last.get() {
            None => true,
            Some((stage, at)) => {
                stage != progress.stage || now.duration_since(at) >= self.control.progress_interval
            }
        };
        if !due {
            return;
        }
        self.last.set(Some((progress.stage, now)));
        progress.round = self.round.get();
        progress.work = self.work.get().saturating_add(pending);
        callback(&progress);
    }
}

#[cfg(any(
    feature = "fpga",
    feature = "mapf",
    feature = "asic",
    feature = "kicad"
))]
impl Run<'_> {
    /// Sets the round that progress reports carry.
    pub(crate) fn set_round(&self, round: u32) {
        self.round.set(round);
    }
}

/// What runs that route with the engine need.
#[cfg(any(feature = "fpga", feature = "mapf", feature = "asic"))]
impl Run<'_> {
    /// Tells of an iteration of negotiation, with `expanded` nodes not yet charged.
    pub(crate) fn tell_iteration(&self, report: &crate::engine::IterationReport, expanded: u64) {
        if self.listening() {
            let mut progress = Progress::new(Stage::Route, u64::from(report.iteration));
            progress.cost = Some(report.total_cost.get());
            progress.conflicts = Some(u64::from(report.overused_resources));
            self.tell(progress, expanded);
        }
    }

    /// The cancel flag to give an engine that runs within [`watch`](Run::watch): one that
    /// is set once the run has to stop, if anything can stop it from outside; otherwise
    /// `own`, the engine options' flag.
    pub(crate) fn engine_flag<'s>(&'s self, own: Option<&'s AtomicBool>) -> Option<&'s AtomicBool> {
        if self.control.cancel.is_some() || self.control.deadline.is_some() || own.is_some() {
            Some(&self.stop)
        } else {
            None
        }
    }

    /// Notes how an engine run within [`watch`](Run::watch) ended: a run whose engine was
    /// cancelled by its options' own flag is one that was cancelled.
    pub(crate) fn engine_stopped(&self, stop: crate::engine::StopReason) {
        if stop == crate::engine::StopReason::Cancelled && self.stopped().is_none() {
            self.halt(Stopped::Cancelled);
        }
    }

    /// Runs `body` while a thread watches the clock, the caller's flag and `own`, an
    /// engine's own flag if it has one, and sets [`engine_flag`](Run::engine_flag)'s flag as
    /// soon as one of them says to stop. With nothing to watch, there is no thread. If no
    /// thread can be started, the run is only stopped where it [polls](Run::poll).
    pub(crate) fn watch<R>(&self, own: Option<&AtomicBool>, body: impl FnOnce() -> R) -> R {
        let (cancel, deadline) = (self.control.cancel, self.control.deadline);
        if cancel.is_none() && deadline.is_none() && own.is_none() {
            return body();
        }
        if own.is_some_and(|f| f.load(Ordering::Relaxed)) {
            self.halt(Stopped::Cancelled);
        }
        if self.poll().is_some() {
            return body();
        }
        let done = AtomicBool::new(false);
        let (stop, why) = (&self.stop, &self.why);
        std::thread::scope(|scope| {
            let done = &done;
            let watcher = std::thread::Builder::new()
                .name("guide-watch".into())
                .spawn_scoped(scope, move || {
                    while !done.load(Ordering::Acquire) {
                        let flagged =
                            |f: Option<&AtomicBool>| f.is_some_and(|f| f.load(Ordering::Relaxed));
                        if flagged(cancel) || flagged(own) {
                            halt_with(stop, why, Stopped::Cancelled);
                            return;
                        }
                        let now = Instant::now();
                        let wait = match deadline {
                            Some(d) if now >= d => {
                                halt_with(stop, why, Stopped::Deadline);
                                return;
                            }
                            Some(d) => (d - now).min(WATCH_INTERVAL),
                            None => WATCH_INTERVAL,
                        };
                        std::thread::park_timeout(wait);
                    }
                });
            let result = body();
            done.store(true, Ordering::Release);
            if let Ok(watcher) = &watcher {
                watcher.thread().unpark();
            }
            result
        })
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_of_nothing_stops_at_once_and_charges_stop_when_spent() {
        let mut control = RunControl::default();
        control.work_budget = Some(0);
        assert_eq!(Run::new(&control).stopped(), Some(Stopped::WorkBudget));
        control.work_budget = Some(10);
        let run = Run::new(&control);
        assert_eq!(run.budget(100), 10);
        run.charge(4);
        assert_eq!((run.stopped(), run.work_left()), (None, Some(6)));
        run.charge(6);
        assert_eq!(run.stopped(), Some(Stopped::WorkBudget));
        // The first reason stays.
        run.halt(Stopped::Cancelled);
        assert_eq!(run.stopped(), Some(Stopped::WorkBudget));
    }

    #[test]
    fn polling_sees_the_flag_and_the_deadline() {
        let flag = AtomicBool::new(false);
        let mut control = RunControl::default();
        control.cancel = Some(&flag);
        let run = Run::new(&control);
        assert_eq!(run.poll(), None);
        flag.store(true, Ordering::Relaxed);
        assert_eq!(run.poll(), Some(Stopped::Cancelled));
        let mut control = RunControl::default();
        control.deadline = Some(Instant::now());
        assert_eq!(Run::new(&control).poll(), Some(Stopped::Deadline));
        assert_eq!(Run::new(&RunControl::default()).poll(), None);
    }

    #[test]
    #[cfg(any(feature = "fpga", feature = "mapf", feature = "asic"))]
    fn a_watcher_sets_the_flag_at_the_deadline_and_stops_with_the_body() {
        let mut control = RunControl::default();
        control.deadline = Some(Instant::now() + Duration::from_millis(20));
        let run = Run::new(&control);
        let flag = run.engine_flag(None).expect("a deadline needs a flag");
        run.watch(None, || {
            while !flag.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
        });
        assert_eq!(run.stopped(), Some(Stopped::Deadline));
        // A body that ends first stops the watcher at once.
        let mut control = RunControl::default();
        control.deadline = Some(Instant::now() + Duration::from_secs(3600));
        let run = Run::new(&control);
        let start = Instant::now();
        assert_eq!(run.watch(None, || 7), 7);
        assert!(start.elapsed() < Duration::from_secs(60));
        assert_eq!(run.stopped(), None);
    }

    #[test]
    fn progress_is_told_at_a_change_of_stage_and_otherwise_by_the_interval() {
        let heard = core::cell::RefCell::new(Vec::new());
        let listen = |p: &Progress| heard.borrow_mut().push((p.stage, p.step, p.work));
        let mut control = RunControl::default();
        control.progress = Some(&listen);
        control.progress_interval = Duration::from_secs(3600);
        let run = Run::new(&control);
        run.charge(5);
        run.tell(Progress::new(Stage::Global, 1), 0);
        run.tell(Progress::new(Stage::Global, 2), 0);
        run.tell(Progress::new(Stage::Anneal, 1), 3);
        assert_eq!(
            *heard.borrow(),
            [(Stage::Global, 1, 5), (Stage::Anneal, 1, 8)]
        );
    }
}
