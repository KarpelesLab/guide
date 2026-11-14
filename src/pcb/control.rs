//! Run control: stopping a run at a deadline or on a flag, hearing of its progress, and the
//! work budget that stops it reproducibly.
//!
//! [`RunControl`] is what a caller hands to [`route_with`](super::route_with): none of it
//! changes what a run that is left to finish returns. `Run` is the router's side of it: one
//! per run, shared by every search of the run through the [`World`](super::world::World).

use core::cell::Cell;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::search::SearchLimits;

/// What a run is doing, for [`Progress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Phase {
    /// Before routing: escapes, pairs routed first, and the zones' fills that nets are
    /// planned on.
    Prepare,
    /// Global routing, over tiles.
    Global,
    /// Detailed routing with rip-up, the first pass.
    Detail,
    /// The passes on the finer lattice, for what the first left.
    Fine,
    /// Cleanup: fewer vias, tracks pulled tight.
    Cleanup,
    /// Differential pairs put side by side in place of their nets' routes.
    Pairs,
    /// Zones filled around the routes, and their nets' pieces joined.
    Zones,
    /// Meanders, for lengths.
    Lengths,
    /// The report of what is left open.
    Report,
}

/// Where a run is, as [`RunControl::progress`] hears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Progress {
    /// What the run is doing.
    pub phase: Phase,
    /// Nets to route. Zero until the nets are known, at the end of [`Phase::Prepare`].
    pub nets: usize,
    /// Of those, the nets that have a route now. Rip-up takes routes out again, so this can
    /// fall.
    pub nets_routed: usize,
    /// Connections to make: as [`Stats::connections`](super::Stats::connections) before
    /// zones are counted.
    pub connections: usize,
    /// Of those, the connections that routes make now.
    pub connected: usize,
    /// Nets waiting for a search, in the pass that is running.
    pub queued: usize,
    /// Work spent so far: the nodes that searches have expanded, the unit of
    /// [`RouteOptions::work_budget`](super::RouteOptions::work_budget).
    pub work: u64,
}

impl Progress {
    fn start() -> Progress {
        Progress {
            phase: Phase::Prepare,
            nets: 0,
            nets_routed: 0,
            connections: 0,
            connected: 0,
            queued: 0,
            work: 0,
        }
    }
}

/// Why a run stopped before it was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Stopped {
    /// [`RouteOptions::work_budget`](super::RouteOptions::work_budget) was spent. The result
    /// is the same for the same budget, on any machine.
    WorkBudget,
    /// [`RunControl::deadline`] passed. The result depends on when.
    Deadline,
    /// [`RunControl::cancel`] was set. The result depends on when.
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

/// What may stop a run from outside, and who hears of its progress.
///
/// Create this with [`Default`] and set the fields. Nothing here changes the result of a
/// run that is left to finish: a progress callback only listens, and a run that a deadline
/// or the flag stops says so in [`Routed::stopped`](super::Routed::stopped). Such a result
/// is valid, and holds what was routed until then, but depends on the moment: the
/// reproducible way to bound a run is
/// [`RouteOptions::work_budget`](super::RouteOptions::work_budget).
///
/// ```
/// use std::sync::atomic::AtomicBool;
/// use std::time::{Duration, Instant};
/// use guide::pcb::RunControl;
///
/// let cancel = AtomicBool::new(false);
/// let heard = |p: &guide::pcb::Progress| eprintln!("{:?}: {} of {}", p.phase, p.connected, p.connections);
/// let mut control = RunControl::default();
/// control.cancel = Some(&cancel);
/// control.deadline = Some(Instant::now() + Duration::from_secs(60));
/// control.progress = Some(&heard);
/// ```
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct RunControl<'a> {
    /// A flag that stops the run once set, from another thread or from the progress
    /// callback. It is read every thousand or so search nodes and between the steps of
    /// every phase, which is within a second but for a fill of large zones, which runs to
    /// its end.
    pub cancel: Option<&'a AtomicBool>,
    /// When to stop, by the clock, read as often as the flag.
    pub deadline: Option<Instant>,
    /// Called at every change of phase, and otherwise no more often than
    /// [`progress_interval`](RunControl::progress_interval), on the thread that routes.
    pub progress: Option<&'a dyn Fn(&Progress)>,
    /// The shortest time between two calls of [`progress`](RunControl::progress) within a
    /// phase. The default is 100 ms.
    pub progress_interval: Duration,
    /// Whether a run stopped by its deadline or its flag still fills the zones around what
    /// it has routed. The default is true. Filling large zones takes seconds, tens of them
    /// on a large board, and runs to its end, so a host that fills zones itself, or that
    /// needs the run back within a second of its deadline, sets this to false: such a run
    /// then has no [fills](super::Routed::fills), gives up a fill it is in the middle of,
    /// and counts its connections on its tracks and vias alone, so that nets joined
    /// through their zones are among its [`unrouted`](super::Routed::unrouted). A run
    /// stopped by its work budget fills its zones in any case, as its result is the same
    /// on any machine.
    pub fill_on_stop: bool,
}

impl Default for RunControl<'_> {
    fn default() -> Self {
        RunControl {
            cancel: None,
            deadline: None,
            progress: None,
            progress_interval: Duration::from_millis(100),
            fill_on_stop: true,
        }
    }
}

impl fmt::Debug for RunControl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunControl")
            .field("cancel", &self.cancel)
            .field("deadline", &self.deadline)
            .field("progress", &self.progress.map(|_| "Fn"))
            .field("progress_interval", &self.progress_interval)
            .field("fill_on_stop", &self.fill_on_stop)
            .finish()
    }
}

/// How many search nodes pass between looks at the clock and the caller's flag.
const POLL_INTERVAL: u32 = 1024;

/// One run's budget, stop flag and progress, shared by all of its searches.
pub(crate) struct Run<'a> {
    control: RunControl<'a>,
    /// Set once the run has to stop, for whatever reason: what searches are given as their
    /// cancel flag.
    stop: AtomicBool,
    why: Cell<Option<Stopped>>,
    /// The work budget left.
    left: Cell<u64>,
    /// Work charged so far.
    work: Cell<u64>,
    /// Nodes expanded by the search that is running, not yet charged.
    pending: Cell<u64>,
    ticks: Cell<u32>,
    state: Cell<Progress>,
    last: Cell<Option<Instant>>,
}

impl<'a> Run<'a> {
    pub(crate) fn new(control: &RunControl<'a>, budget: Option<u64>) -> Run<'a> {
        let run = Run {
            control: *control,
            stop: AtomicBool::new(false),
            why: Cell::new(None),
            left: Cell::new(budget.unwrap_or(u64::MAX)),
            work: Cell::new(0),
            pending: Cell::new(0),
            ticks: Cell::new(0),
            state: Cell::new(Progress::start()),
            last: Cell::new(None),
        };
        if budget == Some(0) {
            run.halt(Stopped::WorkBudget);
        }
        run
    }

    fn halt(&self, why: Stopped) {
        if self.why.get().is_none() {
            self.why.set(Some(why));
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Why the run has to stop, if it has to. Reads no clock and no flag: see
    /// [`poll`](Run::poll).
    pub(crate) fn stopped(&self) -> Option<Stopped> {
        self.why.get()
    }

    /// Whether zones are not to be filled any more: the run was stopped by the clock or
    /// the caller's flag, and the caller asked for no fill then. Looks at both.
    pub(crate) fn no_fill(&self) -> bool {
        !self.control.fill_on_stop
            && self.poll()
            && matches!(self.why.get(), Some(Stopped::Deadline | Stopped::Cancelled))
    }

    /// The flag that is set once the run has to stop, for what takes a flag of its own,
    /// such as global routing.
    pub(crate) fn flag(&self) -> &AtomicBool {
        &self.stop
    }

    /// One node of a search that the budget doesn't count, as global routing's: looks at
    /// the clock and the caller's flag every [`POLL_INTERVAL`].
    #[inline]
    pub(crate) fn glance(&self) {
        let t = self.ticks.get().wrapping_add(1);
        self.ticks.set(t);
        if t.is_multiple_of(POLL_INTERVAL) {
            self.poll();
        }
    }

    /// Work charged so far.
    pub(crate) fn work(&self) -> u64 {
        self.work.get()
    }

    /// The limits of a search that may expand `max` nodes: no more than the budget has
    /// left, and until the run stops.
    pub(crate) fn limits(&self, max: u64) -> SearchLimits<'_> {
        let mut limits = SearchLimits::NONE;
        limits.max_expansions = max.min(self.left.get());
        limits.cancel = Some(&self.stop);
        limits
    }

    /// Charges a search's nodes to the budget, which stops the run when it is spent.
    pub(crate) fn charge(&self, expanded: u64) {
        self.pending.set(0);
        self.work.set(self.work.get().saturating_add(expanded));
        let left = self.left.get().saturating_sub(expanded);
        self.left.set(left);
        if left == 0 {
            self.halt(Stopped::WorkBudget);
        }
    }

    /// One node of a search: looks at the clock and the flag every [`POLL_INTERVAL`].
    #[inline]
    pub(crate) fn tick(&self) {
        self.pending.set(self.pending.get() + 1);
        let t = self.ticks.get().wrapping_add(1);
        self.ticks.set(t);
        if t.is_multiple_of(POLL_INTERVAL) {
            self.poll();
        }
    }

    /// Looks at the caller's flag and the clock, tells of progress if it is time to, and
    /// returns whether the run has to stop.
    pub(crate) fn poll(&self) -> bool {
        // Told first, so that a flag set from the callback stops the run at once.
        self.tell(false);
        if self.why.get().is_none() {
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
        self.why.get().is_some()
    }

    fn tell(&self, always: bool) {
        let Some(progress) = self.control.progress else {
            return;
        };
        let now = Instant::now();
        if !always
            && self
                .last
                .get()
                .is_some_and(|l| now.duration_since(l) < self.control.progress_interval)
        {
            return;
        }
        self.last.set(Some(now));
        let mut state = self.state.get();
        state.work = self.work.get().saturating_add(self.pending.get());
        progress(&state);
    }

    /// Enters a phase, which is always told.
    pub(crate) fn phase(&self, phase: Phase) {
        let mut state = self.state.get();
        if state.phase != phase || self.last.get().is_none() {
            state.phase = phase;
            self.state.set(state);
            self.tell(true);
        }
    }

    /// The nets and connections to route.
    pub(crate) fn totals(&self, nets: usize, connections: usize) {
        let mut state = self.state.get();
        (state.nets, state.connections) = (nets, connections);
        self.state.set(state);
    }

    /// Where the nets are now.
    pub(crate) fn counts(&self, nets_routed: usize, connected: usize, queued: usize) {
        let mut state = self.state.get();
        (state.nets_routed, state.connected, state.queued) = (nets_routed, connected, queued);
        self.state.set(state);
    }
}

/// The limits of a search of `max` nodes in a run, or outside of any.
pub(crate) fn limits<'r>(run: Option<&'r Run<'_>>, max: u64) -> SearchLimits<'r> {
    match run {
        Some(run) => run.limits(max),
        None => {
            let mut limits = SearchLimits::NONE;
            limits.max_expansions = max;
            limits
        }
    }
}
