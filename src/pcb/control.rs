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

use super::zones::Fill;
use crate::geom::drc::{Track, Via};
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
    /// What the passes left open, a connection at a time, with the nets in its way moved
    /// and joined up again, kept if the board has more connections for it.
    Settle,
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
    /// Every net routed against every other at once, on one grid, before the first pass
    /// ([`RouteOptions::whole`](super::RouteOptions::whole)).
    Whole,
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

/// Why a run stopped before it was done: the crate's [`Stopped`](crate::control::Stopped),
/// which every domain's runs share. [`RouteOptions::work_budget`](super::RouteOptions::work_budget)
/// is PCB routing's work budget, [`RunControl::deadline`] its deadline and
/// [`RunControl::cancel`] its flag.
pub use crate::control::Stopped;

/// What may stop a run from outside, and who hears of its progress.
///
/// Create this with [`Default`] and set the fields. Nothing here but
/// [`refill`](RunControl::refill) changes the result of a
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
    /// When to stop, by the clock, read as often as the flag. A run with a deadline routes
    /// [for the most connections soonest](super#stopping-a-run-and-what-is-left-open), as
    /// one with a work budget does, whether the deadline stops it or not.
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
    /// and counts its connections on the last fill it made: the one that the joining of
    /// zones' pieces ended on, or the one that routing planned on, before anything was
    /// routed, on which a net that routes have cut since still counts as joined. A run
    /// stopped before any fill counts on its tracks and vias alone, and nets joined
    /// through their zones are then among its [`unrouted`](super::Routed::unrouted). A run
    /// stopped by its work budget fills its zones in any case, as its result is the same
    /// on any machine.
    pub fill_on_stop: bool,
    /// The host's own fill of its zones around routes, for a host that
    /// [fills its zones itself](super::Zones::fills): given the tracks and the vias of the
    /// run as they stand ([`Refill`]), it returns the fills of the zones as its board
    /// would then have them, each [`Fill`] one connected piece on one layer, or `None` if
    /// it has none to give now, as when its own time is short.
    ///
    /// A run with a [budget](super::RouteOptions::work_budget) or a deadline that
    /// [tells the pads its routes cut off their fills](super::RouteOptions::pour_cuts)
    /// estimates those fills on a grid while it routes: an estimate that takes a place
    /// between two tracks for filled where the host's fill, with its least width and its
    /// islands, leaves none. With this, a board with fills on an outer layer, which
    /// routes cut, has the pads cut off told by the host's fills instead, exactly: a pad
    /// that its zone's fill touches is joined to it, and no other is. They are asked for
    /// at most three times while
    /// [every net is negotiated at once](super::RouteOptions::whole), and at most three
    /// times once routing is over. And on any board they are asked for once more when
    /// cleanup is over ([`Refill::last`]): the stubs to a plane that those fills make
    /// needless, as a pour of an outer layer joins their pads to the net, are taken out
    /// again ([`Stats::stubs_pruned`](super::Stats::stubs_pruned)) and returned apart
    /// ([`Routed::spare`](super::Routed::spare)), and a
    /// result with no [fills](super::Routed::fills) of its own is counted on them.
    /// Where it returns `None`, and between those times, the estimate stands. The call
    /// is made on the thread that routes; the run waits for it, and its time is the
    /// run's, also past a deadline for the last one. A run is as reproducible as what
    /// this returns.
    pub refill: Option<&'a RefillFn<'a>>,
}

/// A host's fill of its zones around a run's routes ([`RunControl::refill`]).
pub type RefillFn<'a> = dyn Fn(&Refill<'_>) -> Option<Vec<Fill>> + 'a;

/// What a run asks its host's fill for ([`RunControl::refill`]): the copper that the
/// [`Design`](super::Design) does not have.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Refill<'a> {
    /// The tracks of the run as they stand: its routes, and those of an
    /// [`Earlier`](super::Earlier) run, moved or not.
    pub tracks: &'a [Track],
    /// Its vias, likewise.
    pub vias: &'a [Via],
    /// Whether routing and cleanup are over: these fills are then also what the stubs
    /// that they make needless are told by, and what the result is counted on. A run
    /// asks so once, also when its deadline has passed.
    pub last: bool,
}

impl Default for RunControl<'_> {
    fn default() -> Self {
        RunControl {
            cancel: None,
            deadline: None,
            progress: None,
            progress_interval: Duration::from_millis(100),
            fill_on_stop: true,
            refill: None,
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
            .field("refill", &self.refill.map(|_| "Fn"))
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
    /// Whether it has a budget of work.
    budgeted: bool,
    /// How long the fill that nets were planned on took, if one was made and timed.
    fill_took: Cell<Option<Duration>>,
    /// When the part of the run that is under way has to end, for the parts after it to
    /// have their share of the deadline, and at how much work, for their share of the
    /// budget.
    part_until: Cell<Option<Instant>>,
    part_work: Cell<u64>,
    /// Whether that part has to end now.
    paused: Cell<bool>,
    /// What ended a part early, if anything did: the run is then one that was stopped, when
    /// it ends.
    cut: Cell<Option<Stopped>>,
    /// When the run began.
    began: Instant,
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
            budgeted: budget.is_some(),
            fill_took: Cell::new(None),
            part_until: Cell::new(None),
            part_work: Cell::new(u64::MAX),
            paused: Cell::new(false),
            cut: Cell::new(None),
            began: Instant::now(),
        };
        if budget == Some(0) {
            run.halt(Stopped::WorkBudget);
        }
        run
    }

    /// Moves the deadline, if there is one, to `share` of the time from the run's beginning
    /// to it: for a run that begins again as one with less to do.
    pub(crate) fn shorten(&mut self, share: f64) {
        let began = self.began;
        if let Some(d) = self.control.deadline.as_mut() {
            *d = began
                + d.saturating_duration_since(began)
                    .mul_f64(share.clamp(0.0, 1.0));
        }
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

    /// Whether the run has a budget of work or a deadline: it may be stopped before its
    /// end, by other than its caller's flag.
    pub(crate) fn bounded(&self) -> bool {
        self.control.deadline.is_some() || self.budgeted
    }

    /// Whether zones are not to be filled any more: the run was stopped by the clock or
    /// the caller's flag, and the caller asked for no fill then. Looks at both.
    pub(crate) fn no_fill(&self) -> bool {
        !self.control.fill_on_stop
            && self.poll()
            && matches!(self.why.get(), Some(Stopped::Deadline | Stopped::Cancelled))
    }

    /// Begins a part of the run that may have `share` of the time that the deadline leaves
    /// now, and `work_share` of the work that the budget leaves, and no more: it
    /// ends then, so that the parts after it have the rest. A share of 1,
    /// or a run with neither, is a part with no end of its own.
    pub(crate) fn part(&self, share: f64, work_share: f64) {
        self.paused.set(false);
        self.stop.store(self.why.get().is_some(), Ordering::Relaxed);
        let whole = share >= 1.0 && work_share >= 1.0;
        self.part_until
            .set(self.control.deadline.filter(|_| !whole).map(|d| {
                let now = Instant::now();
                now + d.saturating_duration_since(now).mul_f64(share)
            }));
        self.part_work.set(if self.budgeted && !whole {
            self.work
                .get()
                .saturating_add((self.left.get() as f64 * work_share) as u64)
        } else {
            u64::MAX
        });
        self.pause_if_due(false);
    }

    /// Ends the part under way if its share is spent, by work, or by the clock if `clock`.
    fn pause_if_due(&self, clock: bool) {
        if self.why.get().is_some() || self.paused.get() {
            return;
        }
        let why = if self.work.get() >= self.part_work.get() {
            Some(Stopped::WorkBudget)
        } else if clock && self.part_until.get().is_some_and(|t| Instant::now() >= t) {
            Some(Stopped::Deadline)
        } else {
            None
        };
        if why.is_some() {
            self.paused.set(true);
            self.cut.set(self.cut.get().or(why));
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    /// The host's fill of its zones around these routes, if it gives one.
    pub(crate) fn refill(&self, tracks: &[Track], vias: &[Via], last: bool) -> Option<Vec<Fill>> {
        let asked = Refill { tracks, vias, last };
        self.control.refill.and_then(|f| f(&asked))
    }

    /// Whether the caller's flag stopped the run. Looks at it.
    pub(crate) fn cancelled(&self) -> bool {
        self.poll();
        self.why.get() == Some(Stopped::Cancelled)
    }

    /// Whether the host gives fills of its own.
    pub(crate) fn refills(&self) -> bool {
        self.control.refill.is_some()
    }

    /// Whether the deadline, if there is one, leaves less than `needed`.
    pub(crate) fn short_of(&self, needed: Duration) -> bool {
        self.control
            .deadline
            .is_some_and(|d| d.saturating_duration_since(Instant::now()) < needed)
    }

    /// Notes how long a fill of the zones took, and says how long the last one did.
    pub(crate) fn fill_took(&self, took: Option<Duration>) -> Option<Duration> {
        if took.is_some() {
            self.fill_took.set(took);
        }
        self.fill_took.get()
    }

    /// Stops the run as one whose deadline left no time for what it had still to do.
    pub(crate) fn out_of_time(&self) {
        self.halt(Stopped::Deadline);
    }

    /// Whether the run or the part of it under way has to end.
    pub(crate) fn ending(&self) -> bool {
        self.why.get().is_some() || self.paused.get()
    }

    /// Ends the run as one that was stopped, if a part of it was ended early: what that
    /// part had not got to is left undone, as in a run stopped outright.
    pub(crate) fn settle(&self) {
        self.paused.set(false);
        self.part_until.set(None);
        self.part_work.set(u64::MAX);
        match self.cut.get() {
            Some(why) => self.halt(why),
            None => self.stop.store(self.why.get().is_some(), Ordering::Relaxed),
        }
    }

    /// How long the part under way has left, if it has a time to end at.
    pub(crate) fn part_left(&self) -> Option<Duration> {
        self.part_until
            .get()
            .or(self.control.deadline)
            .map(|t| t.saturating_duration_since(Instant::now()))
    }

    /// Gives the part under way, which was ended at its time or its work, a share of what
    /// the run has left on top of what it had, and takes back that it was ended.
    pub(crate) fn extend(&self, share: f64, work_share: f64, was: Option<Stopped>) {
        if self.why.get().is_some() {
            return;
        }
        self.cut.set(was);
        self.part(share, work_share);
    }

    /// How long the deadline leaves, if there is one.
    pub(crate) fn left(&self) -> Option<Duration> {
        self.control
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// Gives the part under way, which was ended at its time or its work, `time` more
    /// and `work` more, no later than the deadline and no more than the budget has, and
    /// takes back that it was ended.
    pub(crate) fn allow(&self, time: Duration, work: u64, was: Option<Stopped>) {
        if self.why.get().is_some() {
            return;
        }
        self.cut.set(was);
        self.paused.set(false);
        self.stop.store(false, Ordering::Relaxed);
        self.part_until.set(self.control.deadline.map(|d| {
            let now = Instant::now();
            (now + time).min(d)
        }));
        self.part_work.set(if self.budgeted {
            self.work.get().saturating_add(work.min(self.left.get()))
        } else {
            u64::MAX
        });
        self.pause_if_due(false);
    }

    /// What ended a part early, if anything did.
    pub(crate) fn cut(&self) -> Option<Stopped> {
        self.cut.get()
    }

    /// Ends a part that leaves what it had not got to for the parts after it: the run is
    /// stopped only if it was before, or if a part before this one was ended early, which
    /// `was` says.
    pub(crate) fn resume(&self, was: Option<Stopped>) {
        self.paused.set(false);
        self.part_until.set(None);
        self.part_work.set(u64::MAX);
        self.cut.set(was);
        self.stop.store(self.why.get().is_some(), Ordering::Relaxed);
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

    /// The work that the budget leaves, if the run has one.
    pub(crate) fn work_left(&self) -> Option<u64> {
        self.budgeted.then(|| self.left.get())
    }

    /// Work charged so far.
    pub(crate) fn work(&self) -> u64 {
        self.work.get()
    }

    /// The limits of a search that may expand `max` nodes: no more than the budget has
    /// left, and until the run stops.
    pub(crate) fn limits(&self, max: u64) -> SearchLimits<'_> {
        let mut limits = SearchLimits::NONE;
        let part = self.part_work.get().saturating_sub(self.work.get());
        limits.max_expansions = max.min(self.left.get()).min(part);
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
        self.pause_if_due(false);
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
        self.pause_if_due(true);
        self.ending()
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
    limits_or(run, None, max)
}

/// The limits of a search of `max` nodes in a run, or outside of any until `halt` is set.
pub(crate) fn limits_or<'r>(
    run: Option<&'r Run<'_>>,
    halt: Option<&'r AtomicBool>,
    max: u64,
) -> SearchLimits<'r> {
    match run {
        Some(run) => run.limits(max),
        None => {
            let mut limits = SearchLimits::NONE;
            limits.max_expansions = max;
            limits.cancel = halt;
            limits
        }
    }
}
