//! Multi-net routing engines.
//!
//! [`Engine`] is a negotiated-congestion router, after PathFinder (McMurchie and Ebeling,
//! 1995). Nets may share resources at first, at a price, and the price rises until they stop.
//! [`route`] runs one to the end, [`route_parallel`] does the same with more threads, and
//! stepping an [`Engine`] yourself shows the negotiation one iteration at a time.
//!
//! # The negotiation
//!
//! Each iteration rips up nets and routes them again, one at a time, each seeing what the
//! others hold. A step into resource `r` costs
//!
//! ```text
//! (base + history(r)) * (1 + present * overuse(r))
//! ```
//!
//! where `base` is the step's cost in the space and `overuse(r)` is how far beyond its
//! capacity `r` would be with this net added. `present` grows each iteration up to a cap, so
//! that sharing gets ever less attractive, and `history(r)` grows each iteration by
//! `history_factor` times the overuse of `r`, so that a resource that keeps being fought
//! over stays dear even at the moments when it is free. That is what stops two nets from
//! taking turns on the same resource for ever. A step that consumes several resources adds
//! up their histories and their overuses. See [`EngineOptions`] for the schedule.
//!
//! The formula is additive in history, unlike the multiplicative form that some routers
//! use, because a zero-cost step has to be able to get dearer: multiplying a base of zero
//! gives zero, and nets would share free resources for ever. A consequence is that
//! `history_factor` is in the units of the space's costs.
//!
//! The first iteration routes every net. After that, only nets that hold an overused
//! resource at the moment their turn comes up are routed again, unless
//! [`reroute_all`](EngineOptions::reroute_all) is set. A net whose conflict was settled by
//! an earlier net that same iteration is left alone.
//!
//! Prices only ever raise a step's cost, so a space's lower bound stays admissible, and they
//! take no step away, so whether a path exists never depends on them.
//!
//! # Multi-terminal nets
//!
//! A net's tree grows from its driver, the first terminal. The first search starts from the
//! driver's access points, at their access costs plus whatever occupying them is priced at,
//! and every later one from all the nodes of the tree at no cost. Each search looks for the
//! nearest access point of any terminal not yet connected. An access point's cost is charged
//! at the end of the search, through [`Target`], so a cheap access point further away can
//! beat a dear one nearby, and the access cost plays its part in choosing a route rather
//! than only being reported. A terminal is connected as soon as the tree touches any of
//! its access points, even when a path to another terminal happens to pass through.
//!
//! The choice of root can be wrong in a directed space: a driver's access point might reach
//! the nearest terminal but not the next. So a net whose tree gets stuck starts again without
//! that root, which it remembers, as prices cannot make it viable later. A net is reported
//! as [`NoPath`](UnroutedReason::NoPath) only when no access point of its driver reaches all
//! of its terminals, and is not searched again after that.
//!
//! Nets with fewer than two terminals get an empty route, which occupies nothing.
//!
//! An access point on a node outside the space is ignored. A terminal with no other kind
//! of access point cannot be connected by any route, so its net is reported as
//! [`NoPath`](UnroutedReason::NoPath) from the start, even if it has no other terminal,
//! and the other nets are routed as if it were not there.
//!
//! # Occupancy
//!
//! A net counts once per resource, however many of its edges consume it, and that includes
//! what its root occupies, through a call to
//! [`resources`](RoutingSpace::resources) with the root as both ends. While a tree grows,
//! resources it already holds are charged at nothing but the step's base cost, since using
//! them again takes nothing more from anyone.
//!
//! # Non-configurable steps
//!
//! A space can tie nodes together through
//! [`non_configurable`](RoutingSpace::non_configurable), as VPR's non-configurable edges do for
//! shorted wires: a route that enters one node of a set has all of the nodes that its steps
//! lead to, as many steps on as they go. The engine handles them in three places, and only
//! for a space whose [`has_non_configurable`](RoutingSpace::has_non_configurable) is true, so
//! that no other space pays anything for them:
//!
//! - **Pricing.** A step into a node is priced with the resources of every node tied to it,
//!   leaving out the node the step came from. So a search sees what entering a set takes from
//!   others before it decides, from the first iteration, as VPR's `get_rr_cong_cost` does.
//! - **Trees.** When a path joins the tree, each new node brings along every node tied to it
//!   that the tree does not have yet, by the tying steps, which become edges of the route.
//!   They hold their resources, count in the cost, and connect any terminal on them. A later
//!   search can start from them, as from any node of the tree.
//! - **Fixed routes** have to have every node tied to each of their own, or
//!   [`fix_routes`](Engine::fix_routes) refuses them.
//!
//! A search does not know that a set it passed through is already its own further along the
//! same path, so a path that enters a set twice is priced for it twice. This only makes such
//! paths look dearer than they are. The [validator](crate::validate) checks the result: every
//! node of a route has what is tied to it on the route too.
//!
//! # Fixed and pre-routed nets
//!
//! [`Engine::fix_routes`] takes routes that the caller has already decided on, such as clock
//! trees or hand routes, as [`FixedRoute`]s. A fixed route holds its resources from before the
//! first iteration to the end, and other nets pay for them as for anything another net holds.
//! It is never ripped up. Results have it as the net's route, or as the start of it, and
//! [`validate::check_fixed`](crate::validate::check_fixed) checks that it is still there.
//!
//! - A route that reaches every terminal **fixes the net**, which is then never routed.
//! - A route that does not is a **partial pre-route**. The net's tree grows from all of it,
//!   with nothing to pay for what it holds, to the terminals it does not reach. When the net
//!   is routed again, only what the engine added is replaced. A net only routes again for an
//!   overused resource that is not in its fixed part, as moving can't settle the others.
//!
//! A partial pre-route that can't be completed, as no path leads from it to a terminal it
//! misses, is reported as [`NoPath`](UnroutedReason::NoPath), and **its fixed part is given
//! up with it**: a result holds whole routes or none, and the validator would not count a part
//! that no result shows. The same goes for a net with a terminal outside the space. Until a
//! partial net is first completed it is unrouted, and its fixed part is held all the same.
//!
//! ## When fixed routes conflict
//!
//! Fixed routes are taken as they are, even when they overuse something between them, or on
//! their own, such as a resource of capacity 0. That overuse can't be settled, so it is kept
//! apart from the rest:
//!
//! - A run **converges** once the only overuse left is what fixed routes cause by themselves:
//!   the amount by which a resource's fixed routes alone exceed its capacity. It does not run on
//!   to the iteration limit.
//! - History only grows by the rest, which is all of the overuse in a run without fixed routes.
//! - The conflict is **reported, not hidden**. [`Outcome::overused`] lists the resource with
//!   all of its nets, [`OverusedResource::fixed`] says how many hold it through a fixed route,
//!   and [`IterationReport::fixed_overuse`] has the amount. [`Outcome::is_success`] is false,
//!   as the validator would find the overuse too.
//!
//! [`Engine::overused`] shows these conflicts as soon as the routes are fixed, before any
//! iteration.
//!
//! # Budgets and cancellation
//!
//! A run ends at [`max_iterations`](EngineOptions::max_iterations), at
//! [`max_expansions`](EngineOptions::max_expansions), or when the
//! [`cancel`](EngineOptions::cancel) flag is set. The flag is read before every net and
//! during long searches, on every thread. A net interrupted part way through keeps the route
//! it had.
//!
//! There is deliberately no time limit. How far a run gets in a given time depends on the
//! machine and its load, so a time limit would make the result depend on them too. An
//! expansion budget stops at the same point everywhere. A caller that does need a deadline
//! can set the cancel flag from a timer, knowingly giving up reproducibility for that run.
//!
//! # Results
//!
//! [`Outcome`] holds the best iteration seen, chosen by fewest unrouted nets, then least
//! total overuse, then, for a [timing-driven](#timing-driven-routing) run, the timing, then
//! least total cost, with the earliest winning ties. A run that ends
//! with conflicts **keeps the conflicting routes**, and [`Outcome::overused`] lists every
//! overused resource with the nets on it. [`Outcome::is_success`] is true only if every net
//! is routed and nothing is overused, and [`Outcome::legal_routes`] returns the routes only
//! in that case.
//!
//! The alternative was to unroute the nets in conflict. That would discard information a
//! caller can use, such as where the congestion is and which nets are involved, and it would
//! have to pick which of the nets on a resource to sacrifice. It would also need a reason
//! that [`UnroutedReason`] does not have, and calling congestion a spent budget or a missing
//! path would be untrue.
//!
//! # Timing-driven routing
//!
//! [`Engine::with_timing`] and [`route_timed`] weigh delay against cost as VPR's
//! timing-driven router does (Betz, Rose and Marquardt, 1999). They need a space that
//! implements [`DelaySpace`], a [`TimingGraph`] of the design, and the arc of that graph for
//! each [`Connection`], all in a [`TimingDriven`]. A run without them is exactly as it was
//! before timing existed, to the bit, and took the same time within the noise of the machine
//! it was measured on. `docs/benchmarks.md` has that, and what timing does on generated
//! FPGA-like fabrics: a critical path 9% to 20% shorter, for 1% to 4% more wire and 28% to
//! 48% more expansions.
//!
//! Each connection, from a driver to one of its sinks, has a criticality `c`, from a
//! [`TimingAnalysis`] of the routes so far as [`crate::timing`] describes, against one clock
//! or [per clock](crate::timing#clock-domains), and the search for it pays for a step
//!
//! ```text
//! (1 - c) * price + c * delay_cost * delay
//! ```
//!
//! where `price` is the step's price from the negotiation above, `delay` is its delay in
//! picoseconds, and [`delay_cost`](TimingDriven::delay_cost) says what a picosecond is worth.
//! That is VPR's `evaluate_timing_driven_backward_costs` in `connection_router.tpp`, which adds
//! `(1 - criticality) * cong_cost` and `criticality * Tdel` to the cost of reaching a node.
//! reticle's router does the same for each node it enters, with its delays turned into cost by
//! a ratio worked out from the graph. Here that ratio is the caller's, since only the space
//! knows what its costs mean. VPR's costs are in seconds, so on an
//! `RrGraph` of the `fpga` feature it is `1e-12`.
//!
//! A critical connection's access costs are weighted like its prices. A connection whose
//! criticality is 0 is searched for exactly as without timing.
//!
//! ## Connections, not nets
//!
//! A net's sinks can be anything from fully critical to not at all, so a timing-driven net is
//! grown one sink at a time, the most critical first, as VPR's `route_net` does. Each search
//! goes to that one sink, with its own criticality, and the first critical sink gets the first
//! choice of trunk. Every node of the tree so far is a source at what it took to get there,
//! `c * delay_cost` times its delay from the root, which is how VPR starts a search from its
//! route tree, at `criticality * Tdel`. So a critical sink that would join the tree far from
//! the root pays for the delay of getting there, and can go back to the root instead.
//!
//! Sinks of criticality 0 come last, and are connected as they are without timing: each
//! search goes to whichever is nearest. So a run in which nothing is critical gives the same
//! routes as a run without timing, which is tested.
//!
//! The delay of a connection is the sum of the step delays along the tree from the root to the
//! sink, to the fastest of its access points that the tree reaches.
//!
//! ## The lower bound
//!
//! A search of criticality `c` estimates what is left as
//!
//! ```text
//! (1 - c) * lower_bound + c * delay_cost * delay_lower_bound
//! ```
//!
//! from the space's [`lower_bound`](RoutingSpace::lower_bound) and
//! [`delay_lower_bound`](DelaySpace::delay_lower_bound). This is admissible whenever both of
//! those are. Along any path, each price is at least the step's base cost, since prices only
//! ever raise a cost, so the prices add up to at least the cost bound, and the delays to at
//! least the delay bound. Both weights are at least 0, so the blend of the sums is at least the
//! blend of the bounds, and access costs at the end only add to it. The result is shrunk by a
//! millionth of a millionth, as each blended step is rounded on its own.
//!
//! The space's cost bound on its own, unscaled, would not do. At a criticality of 0.99 a step
//! that costs 1 and takes no time counts as 0.01, and a bound of 1 for it would be a hundred
//! times too high. With the default delay bound of 0, the blend is still admissible, but a
//! critical search then has little to go on, so a space with a sense of distance should give
//! one. A property test checks the bound against true distances, with prices from history and
//! overuse, at any criticality, and on bounds both as tight as they can be and looser.
//!
//! ## The loop
//!
//! Criticalities are worked out once before the first iteration, from the delays the timing
//! graph was built with, which should be the caller's best estimates. After each iteration,
//! every routed connection's delay goes into the timing graph through
//! [`apply_net_delays`](TimingGraph::apply_net_delays), the analysis runs again, and
//! criticalities are refreshed. A connection whose net has no route keeps its last delay. Every
//! [`IterationReport`] has what the analysis found, critical-path delay included, in
//! [`timing`](IterationReport::timing).
//!
//! Criticalities do not change during an iteration, so every net of an iteration sees the
//! same ones, whatever its batch and thread. That keeps the thread count out of the result as
//! it is without timing, which is tested on 1 to 8 threads.
//!
//! ## Which iteration is best, and when to stop
//!
//! After unrouted nets and overuse, iterations are ranked by worst negative slack, then total
//! negative slack, and only then by cost, which is the order of VPR's
//! `is_better_quality_routing` after legality. Once timing is met, more slack is not better,
//! as in VPR. [`Outcome::timing`] has the timing of the best iteration, with the delay of each
//! connection.
//!
//! By default a run still stops as soon as nothing is overused. VPR does the same by default,
//! with a `--router_max_convergence_count` of 1. A net that holds nothing overused is not
//! rerouted, so a connection that grew slow is only rerouted if its net is in a conflict,
//! unless the run goes on as the next section describes.
//!
//! ## Improving timing after a legal result
//!
//! With [`timing_iterations`](EngineOptions::timing_iterations) above 0, a run goes on for up
//! to that many iterations after its first legal result, to shorten its critical path:
//!
//! - Each of them routes again every net with a connection whose criticality is at least
//!   [`timing_reroute_criticality`](EngineOptions::timing_reroute_criticality), as well as every
//!   net on an overused resource, as always.
//! - After an iteration that leaves nothing overused, the present factor starts again from
//!   [`initial_present`](EngineOptions::initial_present), so that critical nets can take what
//!   others hold, at a price, and the others negotiate their way around them. Without that,
//!   prices are as high as the negotiation left them, a critical net finds the route it had,
//!   and nothing changes. Overuse in between adds history as usual.
//! - The best iteration is kept, ranked as above with the critical-path delay after the
//!   slacks, so that a shorter path counts once timing is met. A legal result is never traded
//!   for one that is not, and the critical path is never longer than at the first legal result.
//! - It stops when the extra iterations run out, when nothing counts as critical, or when a
//!   legal iteration changed no route, since the next would find the same routes again. It
//!   stops as [`Converged`](StopReason::Converged) then, or at
//!   [`max_iterations`](EngineOptions::max_iterations), as there is a legal result. Budgets
//!   and cancellation stop it as they stop any run.
//!
//! This is close in spirit to VPR's `--incremental_reroute_delay_ripup`, which reroutes critical
//! connections that are much slower than they have been, though VPR does it during negotiation.
//!
//! On the generated fabrics of `docs/benchmarks.md`, 18 instances in three groups, 10 extra
//! iterations shortened the critical path of 2 instances, by 7% and 23%, and 30 shortened 5,
//! by 3% to 23%. The others were already at, or close to, the fastest each connection could be
//! on its own. Summed over each group, that is 1.2% and 4.5% shorter on the two narrow groups at
//! 10, and 3.3% and 5.9% at 30, for 1.2 to 2.3 times the expansions of plain timing-driven
//! routing, and nothing on the group with room to spare. That is why it is off by default.
//! At 10 iterations, criticalities of 0.5, 0.8 and 0.9 found the same critical paths, 0.95
//! missed the gain on the small group, and the lower the criticality, the more work it took.
//!
//! ## What a delay is
//!
//! **Each step has a fixed delay**, which is an approximation. The delay along a tree is then
//! the sum of the steps from the root, whatever else hangs off it. VPR's is an Elmore delay
//! over its route tree, in which a node's delay depends on the capacitance downstream of it,
//! which changes as the tree grows, and behind an unbuffered switch on the resistance back to
//! the last buffer. A fixed delay can't know either. On a device whose switches are all
//! buffered, the only part lost is the internal capacitance of the switches a branch adds,
//! which `RrGraph::step_delay`, in the `fpga` feature, says more about. It was
//! chosen because it keeps a search a plain shortest path over fixed step costs, so that the
//! search, its lower bound and its optimality are as they are without timing, and because the
//! router and the timing analysis then agree exactly on every delay. `docs/algorithms.md` has a
//! design for analysing with the tree's Elmore delay while still searching with fixed ones.
//!
//! ## Non-configurable steps
//!
//! A set of [tied nodes](#non-configurable-steps) meets timing in two places. The congestion
//! side is weighted by `1 - c`, so whatever a set is priced at is weighted like any other price,
//! and the bound stays admissible, as that price is at least the base cost. On the delay side, a
//! tied node's delay is that of the path to the node it is tied to, plus the delay of the tying
//! step, and a sink can be reached that way. A search does not count the delay of a tie, which
//! is only there once the path is taken. The members of a set that a path does not go through
//! add load that a fixed delay does not see, which is one more way for it to be an approximation.
//!
//! # Determinism
//!
//! The same space, nets and options give the same [`Outcome`], with any platform's floating
//! point and on any number of threads, as long as nothing sets the cancel flag at an
//! unpredictable moment. Nets go in the order they are given, or a seeded shuffle of it. Hash
//! tables here are looked up and never iterated, resources are put in order before their state
//! is touched, and totals are summed over nets in their given order. Slot numbers play no part:
//! each resource's state is worked on alone, so it makes no difference what order the slots
//! come in.
//!
//! # Batches and threads
//!
//! Routing each net in turn lets each see what the one before it did, and no thread can see a
//! commit that has not happened yet. So threads cannot reproduce that order, and something has
//! to give. Here it is [`batch_size`](EngineOptions::batch_size), a choice with an effect on the
//! result, which the caller makes and which no thread count has any say in:
//!
//! 1. The next `batch_size` nets that want routing make up a batch, and they all route against
//!    the table as it stands, with each net seeing its own old route as gone. Nothing writes to
//!    the table in the meantime, which is what makes it safe to share and the same for all.
//! 2. On one thread, in the order of the nets, each route is then looked at. A net that no
//!    longer wants routing, as an earlier one settled its conflict, drops its route. If an
//!    earlier commit of the iteration has since taken a resource that the route uses, the route
//!    is dropped and the net is routed again in the next batch, which now sees that commit. Any
//!    other route is committed. Routes after a dropped one wait, and fill up the next batch.
//!
//! What goes in a batch only depends on those commits, and a route only depends on the table,
//! so the threads have nothing to decide. They only choose who does the work. The same batches
//! run on one thread for a space that cannot be shared, through [`route`] or [`Engine::step`],
//! which is why those need no `Sync`, and agree with [`route_parallel`] on every count.
//!
//! A batch size of 1 **is** the sequential router, to the bit: a batch of one sees all earlier
//! commits and can have no conflict. It is the default, so threads alone do nothing.
//!
//! A route that is kept was still found without seeing earlier commits of its batch, as long
//! as it does not use what they took. That is what makes this faster than a stricter check,
//! and why its results differ from the sequential router's.
//!
//! ## Why not something else
//!
//! All of these were built or measured on the problems in the tables below.
//!
//! - **Checking everything a search looked at, not only its route.** A search that read nothing
//!   that later changed gives exactly what it would have sequentially, so this reproduces the
//!   sequential router at *any* batch size, which was tried and held to the bit. But searches
//!   read far more than they use. In single runs at 8 threads a sparse grid took 2.3 s against
//!   1.7 s here, and on the congested fabric and the dense grid that check was slower than one
//!   thread (11 s against 9.7 s, and 6.0 s against 5.7 s), where this design took 5.2 s and 3.5 s.
//! - **Batches of nets with disjoint bounding boxes.** These cannot be relied on not to interact:
//!   31% of routes on the congested fabric and 25% on the dense grid leave their terminals' box,
//!   and on the dense grid 76% of pairs in a batch searched what the other one held. Disjoint
//!   boxes are also rare among nets like those, and big searches do not get any smaller, so even
//!   if none of this mattered the sizes of the searches would limit 16 threads to 1.36 times the
//!   speed on the fabric and 1.11 on the grid. On a sparse grid of short nets it could work, with
//!   a limit of 15.8 and nearly no interaction, but it would need the space to offer a notion of
//!   region. This design does not, and it gets the benefit there too.
//!
//! ## Quality and convergence
//!
//! Against a batch size of 1, on the same problems and within 100 iterations, with the ratios as
//! means. The dense grids are each row to a shuffled row, at 32, 48, 64 and 96 rows on 2 layers.
//! The fabrics are those of this crate's benchmarks at narrow channels, and there are 20. The 19
//! grids of short nets all have room to spare. Costs compare problems solved at both batch sizes.
//!
//! | Batch size | Dense: solved | Dense: cost | Dense: expansions | Fabrics: cost | Fabrics: expansions | Short nets: cost | Short nets: expansions |
//! | --- | --- | --- | --- | --- | --- | --- | --- |
//! | 1 | 16 of 24 | 1 | 1 | 1 | 1 | 1 | 1 |
//! | 8 | 19 of 24 | 0.995 | 1.67 | 1.000 | 1.04 | 1.000 | 0.99 |
//! | 32 | 16 of 24 | 0.994 | 2.00 | 1.001 | 1.20 | 1.001 | 1.13 |
//! | 64 | 19 of 24 | 0.996 | 1.92 | 1.002 | 1.39 | 1.002 | 1.19 |
//! | 256 | 20 of 24 | 0.996 | 1.81 | 1.003 | 1.72 | 1.001 | 1.51 |
//!
//! All of the fabrics and grids of short nets were solved at all nine batch sizes tried, in about
//! the same number of iterations on average (0.96 to 1.03 times as many), and no route set cost
//! more than 1.1% over sequential.
//!
//! Dense grids are chaotic. Solved counts run from 16 to 21 of 24 with no pattern, which shows no
//! sign that batches hurt, and 24 problems are too few to say that they help. Single problems
//! cost from 3% less to 5% more, and can take from a fifth to several times the iterations.
//! They do cost expansions: about 40% of searches at a batch size of 32 are thrown away, which is
//! most of what threads could have gained.
//!
//! ## Speed
//!
//! Speedup against a batch size of 1 on one thread, as the median of runs taken in turn, with
//! the lowest and highest in brackets. This was a shared machine of 96 cores and 192 threads at
//! a load between 26 and 82, in 3 to 7 rounds per problem. Any one run could be off by anything
//! from a few percent to 75%, so differences in the second digit mean nothing here.
//!
//! | Problem | Alone | Batch | 2 threads | 4 threads | 8 threads | 16 threads |
//! | --- | --- | --- | --- | --- | --- | --- |
//! | 20 000 short nets on a grid of 4.2M nodes | 4.4 s | 64 | 1.17 (1.13 to 1.25) | 2.07 (1.66 to 2.19) | 2.60 (1.61 to 2.92) | 2.93 (2.56 to 3.19) |
//! | 6000 nets, wide fabric of 1.3M nodes | 48 s | 64 | 1.59 (1.57 to 1.74) | 2.21 (2.20 to 2.51) | 2.52 (2.45 to 2.72) | 2.42 (2.40 to 2.57) |
//! | 4000 nets, narrow fabric of 426 000 nodes | 11 s | 32 | 1.46 (1.43 to 1.47) | 1.87 (1.74 to 1.95) | 2.04 (1.81 to 2.04) | 2.10 (1.88 to 2.10) |
//! | Dense grid of 128 rows | 5.7 s | 32 | 1.51 (1.44 to 1.63) | 1.88 (1.59 to 1.97) | 2.08 (1.66 to 2.25) | 2.19 (2.14 to 2.35) |
//! | Dense grid of 64 rows | 0.40 s | 32 | 1.01 (0.98 to 1.02) | 1.24 (1.21 to 1.30) | 1.36 (1.33 to 1.40) | 1.44 (1.40 to 1.51) |
//! | 800 short nets on a grid of 131 000 nodes | 49 ms | 32 | 1.11 (1.00 to 1.33) | 1.10 (1.00 to 1.34) | 1.27 (1.20 to 1.59) | 1.10 (1.06 to 1.34) |
//! | 300 nets, narrow fabric of 27 000 nodes | 18 ms | 16 | 0.62 (0.59 to 0.68) | 1.04 (0.96 to 1.18) | 1.09 (1.03 to 1.13) | 1.01 (0.96 to 1.07) |
//!
//! So **expect 2 to 3 times the speed, with little to gain beyond 8 threads**, on problems that
//! take seconds. Problems of milliseconds gain nothing and can get slower. Some of the dense
//! grid of 128 rows is not due to threads: that one problem happens to take half the iterations
//! in batches, so it runs 1.50 times as fast on one thread, and it is a single sample of a chaotic
//! process. What threads add to that is the 1.45 at 16 threads.
//!
//! Mostly this is not overhead. A batch is as slow as its biggest search, a net is the unit of
//! work, and a few searches are far bigger than the rest. Going by expansions alone, 16 threads
//! could not beat about 2.5 times the speed of one on the narrow fabric at these batch sizes, or
//! 2.9 on the wide one, and thread counts past that gain nothing. The grid of short nets has
//! a limit of 10.6 and falls short for another reason: each batch starts on cold caches, so its
//! searches took 45% more CPU time than on one thread, or about 20% more with all threads kept
//! to one 8-core group of this machine. That is at batches of about 3 ms. The fabrics, with longer
//! batches, did not show it. Starting and joining threads took 22 us for one and 173 us for 15,
//! so it is no large part of this.
//!
//! A good start is a batch size of 32 and 8 threads. Large batches waste work, and past 64 the
//! limits above stayed level or fell on every problem but the wide fabric.
//! [`IterationReport::discarded`] shows how much is being thrown away.
//!
//! ## Memory
//!
//! **Each thread costs a full set of search state: 44 bytes per node of the space, up to the
//! highest slot it reaches.** That is about 13 GB per thread on a graph of 300 million nodes, on
//! top of what the engine holds anyway, and it is why threads are for the caller to ask for. It
//! is kept from batch to batch and iteration to iteration, and only as many workers are made
//! as a batch can use. On the 20 000 nets above, the peak was 301 MiB on one thread, 831 at 4,
//! 1535 at 8 and 2943 at 16, which is the predicted 176 MiB for each thread added, at 4.2 million
//! nodes. Routes waiting in a batch take a little more.
//!
//! ## What is guaranteed
//!
//! - **Budgets.** Work is charged in the order of the nets, while commits are decided, so that
//!   workers cannot race for what is left. Discarded routes are charged, so that
//!   [`RouteStats::nodes_expanded`](crate::net::RouteStats::nodes_expanded) counts all of the
//!   work of the batches, and never exceeds the budget: a route that would go over stops the run
//!   right there, as if the budget had run out inside it. This stops at the same place at every
//!   thread count. The machine can do more work than that, as other members of the last batch
//!   have searches of their own to finish, up to the rest of the budget each.
//! - **Cancellation.** The flag only affects when a run stops. A run in which it is never set is
//!   not affected in any way by having one. Once set, every thread stops at its next check, and
//!   no more is committed. The routes at that point are correct as far as they go, but which
//!   those are depends on the moment, so that the result of a cancelled run can differ from
//!   one to the next.
//! - **Panics.** A panic in a space's method on any thread reaches the caller, with its own
//!   message, after the other threads have run out of work. Do not use the engine after that.
//!
//! To reproduce the measurements, one configuration per run, where `dense:128:2:1` is rows,
//! layers and seed, `local:1024:4:20000:16:1` is cells along a side, layers, nets, reach and
//! seed, and `fabric:128:1:4000:8:1` is tiles along a side, tracks, nets, reach and seed:
//!
//! ```text
//! GUIDE_PROBLEM=fabric:128:1:4000:8:1 GUIDE_THREADS=8 GUIDE_BATCH=32 cargo test --release --lib \
//!     engine::parallel_tests::measure -- --ignored --nocapture --exact
//! GUIDE_PROBLEM=fabric:128:1:4000:8:1 cargo test --release --lib \
//!     engine::parallel_tests::measure_regions -- --ignored --nocapture --exact
//! ```

use core::cell::RefCell;
use core::fmt;
use std::error::Error;
use std::panic::resume_unwind;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::cost::Cost;
use crate::id::NetId;
use crate::net::{FixedRoute, Net, Route, RouteSet, RouteStats, Terminal, UnroutedReason};
use crate::rng::Rng;
use crate::search::{Search, SearchLimits, SearchOutcome, Source, Target};
use crate::space::{DelaySpace, FastMap, FastSet, NodeSlots, RoutingSpace, Step};
use crate::timing::{ArcId, ArcKind, Constraints, CriticalityOptions, TimingAnalysis, TimingGraph};

/// What an [`Engine`] may do, and when it has to stop.
///
/// Create this with [`Default`] and set the fields. The costs are [`Cost`]s so that they
/// cannot be negative or NaN.
///
/// The factors start from the values of VPR and reticle, whose routers have seen far harder
/// problems than this crate's tests, and differ only where the docs of a field say so.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EngineOptions<'c> {
    /// How many iterations to run at most, which is what ends a run on a problem that cannot
    /// be solved. The default is 100.
    ///
    /// VPR and reticle allow 50 and 40, but with few nets left to route late in a run, more
    /// iterations cost little when they are not needed. On dense grids, 100 instead of 50
    /// solved 71 of 72 problems rather than 58 on the set used for tuning, and 71 rather
    /// than 68 on a fresh set, for 1% to 3% more expansions. The price is paid on a problem
    /// that cannot be solved, which takes about 1.8 times as long to give up on. See
    /// `docs/benchmarks.md` for these results and how to reproduce them.
    pub max_iterations: u32,
    /// How many nodes all searches together may expand. The default is no limit.
    ///
    /// This covers work on every thread, including routes that batches go on to discard, and
    /// counts it in a way that no thread count can change: see the [module
    /// documentation](self#what-is-guaranteed).
    pub max_expansions: u64,
    /// A flag that stops the run once set, from any thread. The default is none.
    pub cancel: Option<&'c AtomicBool>,
    /// The present factor of the first iteration. The default is 0.5, the same as the second.
    ///
    /// VPR and reticle use 0 here, so that each net first takes its own shortest path. reticle
    /// measured searches flooding on a real FPGA without it, because no lower bound knows
    /// what earlier nets have taken. That did not happen on the dense grids and small
    /// island-style fabrics measured here, whose lower bounds are close to exact. Against 0,
    /// a charge from the start took 42% to 62% of the expansions on dense grids and 93% on an
    /// FPGA-like fabric. It gave routes about 3.5% cheaper on dense grids and 1% to 2.5%
    /// cheaper elsewhere, because nets avoid conflicts that they would otherwise create and
    /// then have to resolve. It is worth measuring again on real fabrics, where 0 may turn
    /// out to be the better setting.
    ///
    /// With [timing](self#timing-driven-routing), on the same FPGA-like fabrics, 0 gave a
    /// critical path from as short to 3.4% shorter, for 1.4% to 12% more expansions and more
    /// iterations, with wirelength within 0.3%. That is too mixed a result, on too few
    /// synthetic instances, to change the default for.
    pub first_present: Cost,
    /// The present factor of the second iteration, from which it grows. The default is 0.5,
    /// as in VPR and reticle.
    pub initial_present: Cost,
    /// What the present factor is multiplied by after each iteration from the second. The
    /// default is 1.8, as in reticle, where VPR uses 1.3. On dense grids, 1.3 took 1.2 to 1.4
    /// times the expansions and solved fewer problems within the same number of iterations,
    /// but on multi-terminal grids it solved more, 71 against 67.
    pub present_growth: Cost,
    /// The most the present factor can be. The default is 1000, as in VPR.
    ///
    /// At this level, the overuse term already outweighs anything a net could save by
    /// sharing. Above it, costs lose precision for no change in behaviour.
    pub max_present: Cost,
    /// How much history one unit of overuse adds per iteration. The default is 1, as in VPR
    /// and reticle, which suits a space whose typical step costs about 1. Scale it with the
    /// step costs of other spaces, because history adds to them. On dense grids, both 0.2
    /// and 5 did worse. At zero, a step that costs nothing can never become dear.
    pub history_factor: Cost,
    /// Whether to route every net in every iteration, and not only those holding an overused
    /// resource. The default is false. On dense grids this solved fewer problems within the
    /// default iteration cap, 67 of 72 against 71, for about 1.8 times the expansions, and
    /// its routes were only 0.4% cheaper.
    pub reroute_all: bool,
    /// A seed to shuffle the order of the nets with before each iteration after the first.
    /// The default is none, which keeps the given order throughout.
    ///
    /// This did not help on average, so it is off. On dense grids it took 5% to 28% more
    /// expansions and gave routes up to 1.1% dearer, and on an FPGA-like fabric up to 45% and
    /// 5.8%. Changing only the seed moves dense grids between 65 and 72 problems solved of
    /// 72, so it can be a way out for a problem stuck in a cycle, but a difference of a few
    /// problems between two settings means little.
    pub shuffle_seed: Option<u64>,
    /// How many nets to route at a time against the same state of the resource table. The
    /// default is 1, which is plain sequential routing.
    ///
    /// **This changes results**, and thread counts do not: see the [module
    /// documentation](self#batches-and-threads), which also has what it did to cost, success
    /// and speed. Nothing runs in parallel at the default, so a caller who wants threads has to
    /// choose a batch size too. 32 is a good start. A value of 0 means 1.
    pub batch_size: u32,
    /// How many more iterations a [timing-driven](self#timing-driven-routing) run may take
    /// after its first legal result, rerouting the nets of critical connections to shorten its
    /// critical path. The default is 0, which stops at the first legal result. A run without
    /// timing ignores this.
    ///
    /// The best result is kept, so a legal one is never traded for one that is not. See
    /// [improving timing after a legal result](self#improving-timing-after-a-legal-result),
    /// which has what this did on the fabrics measured.
    pub timing_iterations: u32,
    /// The criticality from which a connection counts as critical in those iterations, so that
    /// its net is routed again. The default is 0.9.
    pub timing_reroute_criticality: f64,
}

impl Default for EngineOptions<'_> {
    fn default() -> Self {
        EngineOptions {
            max_iterations: 100,
            max_expansions: u64::MAX,
            cancel: None,
            first_present: Cost::new(0.5).unwrap_or(Cost::ONE),
            initial_present: Cost::new(0.5).unwrap_or(Cost::ONE),
            present_growth: Cost::new(1.8).unwrap_or(Cost::ONE),
            max_present: Cost::from(1000),
            history_factor: Cost::ONE,
            reroute_all: false,
            shuffle_seed: None,
            batch_size: 1,
            timing_iterations: 0,
            timing_reroute_criticality: 0.9,
        }
    }
}

/// One connection of a net, from its driver to one of its sinks, and the arc of a
/// [`TimingGraph`] that stands for it.
///
/// This is how a caller tells the engine which arc's delay a route decides. See [timing-driven
/// routing](self#timing-driven-routing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Connection {
    /// The net.
    pub net: NetId,
    /// The sink, as an index in the net's terminals. It can't be 0, which is the driver.
    pub terminal: u32,
    /// The net arc of the timing graph for this connection.
    pub arc: ArcId,
}

/// What makes a run timing-driven: a timing graph to analyse the routes with, and how to
/// weigh what it finds against cost. See [timing-driven routing](self#timing-driven-routing).
///
/// Create this with [`new`](TimingDriven::new), and set the other fields if need be.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TimingDriven {
    /// The timing graph. Its net arcs start out with estimated delays, from which the first
    /// iteration's criticalities come, and the engine replaces those of the connections it
    /// routes. Other net arcs keep their delays throughout.
    pub graph: TimingGraph,
    /// What the timing graph is checked against: one clock for every path, or a constraint
    /// per pair of clocks, in which case each connection's criticality is the highest over
    /// the pairs it is on, each against its own constraint. See [clock
    /// domains](crate::timing#clock-domains).
    pub constraints: Constraints,
    /// The connections that routes decide the delays of. A sink that is not listed has a
    /// criticality of 0, and a net arc that is not listed keeps its delay.
    pub connections: Vec<Connection>,
    /// How slack turns into criticality. The default is VPR's.
    pub criticality: CriticalityOptions,
    /// What a picosecond of delay costs, in the units of the space's costs.
    ///
    /// This is what makes delay and cost comparable, and it is up to the space. On an
    /// `RrGraph` of the `fpga` feature, whose costs are in seconds as VPR's are, it is
    /// `1e-12`, which weighs the two as VPR does. A space whose steps cost about 1 per tile
    /// wants about 1 over the delay of a tile.
    pub delay_cost: Cost,
}

impl TimingDriven {
    /// Sets up timing-driven routing, with VPR's criticality options.
    pub fn new(
        graph: TimingGraph,
        constraints: Constraints,
        connections: Vec<Connection>,
        delay_cost: Cost,
    ) -> TimingDriven {
        TimingDriven {
            graph,
            constraints,
            connections,
            criticality: CriticalityOptions::default(),
            delay_cost,
        }
    }
}

/// What the timing analysis said about the routes of an iteration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct TimingMetrics {
    /// The arrival time at the end of the critical path, in picoseconds: the critical-path
    /// delay. `None` if no path is constrained.
    pub critical_path_delay: Option<u64>,
    /// The worst slack, in picoseconds, or `None` if no path is constrained.
    pub worst_slack: Option<i64>,
    /// The total negative slack, in picoseconds: zero if timing is met.
    pub total_negative_slack: i128,
}

impl TimingMetrics {
    /// The worst slack where it is negative, and otherwise 0, which is what VPR ranks by: once
    /// timing is met, more slack is not better.
    fn worst_negative_slack(&self) -> i64 {
        self.worst_slack.map_or(0, |slack| slack.min(0))
    }
}

/// The timing of an [`Outcome`]'s routes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct TimingOutcome {
    /// What the analysis of these routes found.
    pub metrics: TimingMetrics,
    /// The delay of each connection, in the order given in [`TimingDriven::connections`], or
    /// `None` for a connection whose net has no route. The analysis used the estimate it was
    /// given for those.
    pub connection_delays: Vec<Option<u32>>,
}

/// Why an [`Engine`] could not be created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EngineError {
    /// Two nets have the same ID, so their results could not be told apart.
    DuplicateNet {
        /// The repeated ID.
        net: NetId,
    },
    /// A [`Connection`] names a net that is not being routed.
    UnknownNet {
        /// The net.
        net: NetId,
    },
    /// A [`Connection`] names a terminal that is the driver or does not exist.
    NotASink {
        /// The net.
        net: NetId,
        /// The terminal.
        terminal: u32,
    },
    /// A [`Connection`] names an arc that is not a net arc of the timing graph.
    NotANetArc {
        /// The arc.
        arc: ArcId,
    },
    /// Two [`Connection`]s are for the same sink or the same arc.
    DuplicateConnection {
        /// The net of the second one.
        net: NetId,
        /// The terminal of the second one.
        terminal: u32,
    },
    /// A [`FixedRoute`] could not be taken: see [`Engine::fix_routes`].
    InvalidFixedRoute {
        /// The net it is for.
        net: NetId,
        /// What is wrong with it.
        problem: FixedRouteProblem,
    },
}

/// What is wrong with a [`FixedRoute`] given to [`Engine::fix_routes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FixedRouteProblem {
    /// The net is not among those being routed.
    UnknownNet,
    /// The net already has a fixed route.
    Repeated,
    /// The engine has already run an iteration, and fixed routes have to be there from the
    /// start.
    AlreadyStarted,
    /// The route has no nodes, so there is nothing to fix.
    Empty,
    /// The route is not a tree directed away from its root: see [`Route::validate_tree`].
    NotATree,
    /// The net needs routing and the route's root is not an access point of its driver.
    RootNotOnDriver,
    /// The route uses a node that the space does not contain.
    NodeNotInSpace,
    /// The net needs no routing, and the route does not reach its terminal.
    MissingTerminal,
    /// The route has a node without a node that is [tied](RoutingSpace::non_configurable) to
    /// it.
    NonConfigurableMissing,
    /// An edge of the route, by its position in [`Route::edges`], is not a step that the
    /// space offers the net.
    NotAStep {
        /// The edge's position.
        index: usize,
    },
}

impl fmt::Display for FixedRouteProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixedRouteProblem::UnknownNet => f.write_str("the net is not being routed"),
            FixedRouteProblem::Repeated => f.write_str("the net already has a fixed route"),
            FixedRouteProblem::AlreadyStarted => f.write_str("the engine has already started"),
            FixedRouteProblem::Empty => f.write_str("the route is empty"),
            FixedRouteProblem::NotATree => f.write_str("the route is not a tree"),
            FixedRouteProblem::RootNotOnDriver => {
                f.write_str("the route does not start at the net's driver")
            }
            FixedRouteProblem::NodeNotInSpace => {
                f.write_str("the route uses a node outside the space")
            }
            FixedRouteProblem::NonConfigurableMissing => {
                f.write_str("the route leaves out a node that is tied to one of its own")
            }
            FixedRouteProblem::MissingTerminal => {
                f.write_str("the route does not reach the net's terminal")
            }
            FixedRouteProblem::NotAStep { index } => {
                write!(f, "edge {index} of the route is not a step of the space")
            }
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::DuplicateNet { net } => write!(f, "net {} is listed twice", net.0),
            EngineError::UnknownNet { net } => {
                write!(f, "a connection names net {}, which is not routed", net.0)
            }
            EngineError::NotASink { net, terminal } => {
                write!(f, "terminal {terminal} of net {} is not a sink", net.0)
            }
            EngineError::NotANetArc { arc } => {
                write!(f, "arc {} is not a net arc of the timing graph", arc.0)
            }
            EngineError::DuplicateConnection { net, terminal } => write!(
                f,
                "terminal {terminal} of net {} repeats a sink or an arc of another connection",
                net.0
            ),
            EngineError::InvalidFixedRoute { net, problem } => {
                write!(f, "the fixed route of net {}: {problem}", net.0)
            }
        }
    }
}

impl Error for EngineError {}

/// Why an [`Engine`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StopReason {
    /// Nothing is overused, so there is nothing left to negotiate. Nets with no path may
    /// still be unrouted: see [`Outcome::is_success`]. Overuse that [fixed
    /// routes](self#fixed-and-pre-routed-nets) cause among themselves can't be negotiated, and
    /// may be left. A run that [improves timing](self#improving-timing-after-a-legal-result)
    /// after a legal result stops this way once it is done, whatever its last iteration did.
    Converged,
    /// [`max_iterations`](EngineOptions::max_iterations) ran out with conflicts left.
    IterationLimit,
    /// [`max_expansions`](EngineOptions::max_expansions) ran out.
    BudgetExhausted,
    /// The [`cancel`](EngineOptions::cancel) flag was set.
    Cancelled,
    /// The engine has not stopped: the caller asked for its result before it finished.
    NotFinished,
}

/// What one iteration of an [`Engine`] did, and the state it left behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct IterationReport {
    /// The number of this iteration, from 1.
    pub iteration: u32,
    /// The present factor used.
    pub present: Cost,
    /// Whether the iteration got through all of its nets, as opposed to being interrupted.
    pub completed: bool,
    /// How many nets were routed again.
    pub nets_rerouted: u32,
    /// How many searches ran.
    pub searches: u64,
    /// How many nodes those searches expanded.
    pub nodes_expanded: u64,
    /// How many nets that need a route have none.
    pub unrouted: u32,
    /// How many resources are overused.
    pub overused_resources: u32,
    /// The sum over resources of how far each is beyond its capacity.
    pub total_overuse: u64,
    /// The part of [`total_overuse`](IterationReport::total_overuse) that [fixed
    /// routes](self#fixed-and-pre-routed-nets) cause among themselves, which no routing can
    /// remove. It is 0 without fixed routes.
    pub fixed_overuse: u64,
    /// The total cost of all routes, as defined at [`Outcome::total_cost`].
    pub total_cost: Cost,
    /// How many batches of searches ran, which at a batch size of 1 is one for each net.
    pub batches: u32,
    /// How many routes from those batches were thrown away, because an earlier net of the
    /// iteration had taken something they use or had left them with nothing to resolve. Their
    /// expansions are in [`nodes_expanded`](IterationReport::nodes_expanded).
    pub discarded: u32,
    /// What timing analysis found of the routes the iteration left, or `None` if the run is
    /// not timing-driven.
    pub timing: Option<TimingMetrics>,
}

/// A resource with more nets on it than its capacity allows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct OverusedResource<R> {
    /// The resource.
    pub resource: R,
    /// How many nets hold it, which is more than `capacity`.
    pub occupancy: u32,
    /// How many nets it can take.
    pub capacity: u32,
    /// The nets that hold it, in ascending order.
    pub nets: Vec<NetId>,
    /// How many of those hold it through the fixed part of their route, and so cannot
    /// leave it. If this is above `capacity`, the conflict is between [fixed
    /// routes](self#fixed-and-pre-routed-nets) and no routing can settle it.
    pub fixed: u32,
}

/// The result of a run: the best iteration seen, and the plain truth about it.
///
/// **Check [`is_success`](Outcome::is_success) before trusting the routes.** A run that stops
/// with conflicts keeps its routes, so [`routes`](Outcome::routes) can look complete while
/// [`overused`](Outcome::overused) says that they are not legal. See the
/// [module documentation](self) for why.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[must_use = "a result may hold conflicts, which only checking it will show"]
pub struct Outcome<N, R> {
    /// A route or a reason for every net. Its statistics cover the whole run, not only the
    /// iteration these routes come from.
    pub routes: RouteSet<N>,
    /// Every resource that is overused by these routes, in ascending order.
    pub overused: Vec<OverusedResource<R>>,
    /// Why the engine stopped.
    pub stop: StopReason,
    /// The iteration these routes come from, from 1, or 0 if no iteration ran.
    pub best_iteration: u32,
    /// The total base cost of these routes. For each routed net in turn, this is the access
    /// cost of its root, then for each path the costs of the new steps in the space, before
    /// any pricing, and the lowest access cost among the access points through which that
    /// path connected each terminal.
    pub total_cost: Cost,
    /// The timing of these routes, or `None` if the run is not timing-driven.
    pub timing: Option<TimingOutcome>,
}

impl<N, R> Outcome<N, R> {
    /// Returns whether every net is routed and no resource is overused.
    pub fn is_success(&self) -> bool {
        self.overused.is_empty() && self.routes.unrouted().next().is_none()
    }

    /// Returns the routes if they are all there and legal, and otherwise `None`.
    pub fn legal_routes(&self) -> Option<&RouteSet<N>> {
        self.is_success().then_some(&self.routes)
    }

    /// Returns the nets involved in a conflict, in ascending order.
    pub fn conflicted_nets(&self) -> Vec<NetId> {
        let mut nets: Vec<NetId> = self
            .overused
            .iter()
            .flat_map(|entry| entry.nets.iter().copied())
            .collect();
        nets.sort_unstable();
        nets.dedup();
        nets
    }
}

/// Routes `nets` through `space` to the end, under `options`.
///
/// This is [`Engine::new`] followed by [`Engine::run`].
///
/// # Errors
///
/// Returns an error if two nets have the same ID.
///
/// # Examples
///
/// Two nets that want to cross on a board of two layers. Each takes its straight line at
/// first, which conflicts where they meet, and the negotiation settles it with a via.
///
/// ```
/// use guide::engine::{EngineOptions, StopReason, route};
/// use guide::id::NetId;
/// use guide::net::{Net, Terminal};
/// use guide::space::grid::Grid;
///
/// let grid = Grid::new(5, 5, 2)?;
/// let at = |x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
/// let nets = [
///     Net { id: NetId(0), terminals: vec![at(0, 2), at(4, 2)] },
///     Net { id: NetId(1), terminals: vec![at(2, 0), at(2, 4)] },
/// ];
///
/// let outcome = route(&grid, &nets, EngineOptions::default()).unwrap();
///
/// assert_eq!(outcome.stop, StopReason::Converged);
/// let routes = outcome.legal_routes().expect("this problem can be routed");
/// assert_eq!(routes.routes().count(), 2);
/// assert!(routes.stats.iterations >= 2);
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
pub fn route<S: RoutingSpace>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
) -> Result<Outcome<S::Node, S::Resource>, EngineError> {
    Ok(Engine::new(space, nets, options)?.run())
}

/// Routes `nets` through `space` to the end, under `options`, with up to `threads` threads
/// searching at once.
///
/// The result is the same as that of [`route`] with the same options, for every thread count
/// and on every platform. A value of 0 means 1. Threads only help if
/// [`batch_size`](EngineOptions::batch_size) is above 1, and the [module
/// documentation](self#batches-and-threads) has the speed they gain.
///
/// **Each thread takes 44 bytes per node of the space**, or about 13 GB per thread on a graph of
/// 300 million nodes. See [memory](self#memory).
///
/// The bounds are here and not on [`RoutingSpace`], so that a space that cannot be shared still
/// has [`route`], and gets the same batches and the same results from it on one thread.
///
/// # Errors
///
/// Returns an error if two nets have the same ID.
///
/// # Panics
///
/// A panic in a space's methods on any thread reaches the caller.
///
/// # Examples
///
/// ```
/// use guide::engine::{EngineOptions, route, route_parallel};
/// use guide::id::NetId;
/// use guide::net::{Net, Terminal};
/// use guide::space::grid::Grid;
///
/// let grid = Grid::new(8, 8, 2)?;
/// let at = |x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
/// let nets: Vec<_> = (0..4)
///     .map(|i| Net { id: NetId(i), terminals: vec![at(0, 2 * i), at(7, 6 - 2 * i)] })
///     .collect();
///
/// let mut options = EngineOptions::default();
/// options.batch_size = 2;
///
/// let on_four = route_parallel(&grid, &nets, options, 4).unwrap();
/// assert!(on_four.is_success());
/// assert_eq!(on_four, route(&grid, &nets, options).unwrap());
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
pub fn route_parallel<S>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
    threads: usize,
) -> Result<Outcome<S::Node, S::Resource>, EngineError>
where
    S: RoutingSpace + Sync,
    S::Node: Send + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Sync,
{
    Ok(Engine::new(space, nets, options)?.run_parallel(threads))
}

/// Routes `nets` through `space` to the end, under `options`, weighing delay against cost as
/// `timing` says. See [timing-driven routing](self#timing-driven-routing).
///
/// This is [`Engine::with_timing`] followed by [`Engine::run`].
///
/// # Errors
///
/// Returns an error if two nets have the same ID, or if a connection is not valid: see
/// [`EngineError`].
pub fn route_timed<S: DelaySpace>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
    timing: TimingDriven,
) -> Result<Outcome<S::Node, S::Resource>, EngineError> {
    Ok(Engine::with_timing(space, nets, options, timing)?.run())
}

/// Routes as [`route_timed`] does, with up to `threads` threads searching at once, and the same
/// result for every thread count. See [`route_parallel`].
///
/// # Errors
///
/// As for [`route_timed`].
///
/// # Panics
///
/// A panic in a space's methods on any thread reaches the caller.
pub fn route_timed_parallel<S>(
    space: &S,
    nets: &[Net<S::Node>],
    options: EngineOptions<'_>,
    timing: TimingDriven,
    threads: usize,
) -> Result<Outcome<S::Node, S::Resource>, EngineError>
where
    S: DelaySpace + Sync,
    S::Node: Send + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Sync,
{
    Ok(Engine::with_timing(space, nets, options, timing)?.run_parallel(threads))
}

/// The negotiation state of one resource.
///
/// All of it is in one place because a step reads all of it, so that this is one cache miss
/// per step rather than one per field.
#[derive(Clone, Copy)]
struct ResourceState {
    /// How many nets hold the resource.
    occupancy: u32,
    /// Copied here because a space's answer could mean a cache miss on every step. Only valid
    /// once the resource has been occupied, which `ResourceTable::read` takes care of.
    capacity: u32,
    history: Cost,
}

impl ResourceState {
    /// The state of a slot that no net has occupied.
    const UNOCCUPIED: ResourceState = ResourceState {
        occupancy: 0,
        capacity: 0,
        history: Cost::ZERO,
    };

    /// How far beyond capacity the resource is.
    #[inline]
    fn overuse(self) -> u32 {
        self.occupancy.saturating_sub(self.capacity)
    }
}

/// How far beyond capacity a resource would be with one more net on it.
#[inline]
fn overuse_with_one_more(occupancy: u32, capacity: u32) -> u32 {
    occupancy.saturating_add(1).saturating_sub(capacity)
}

/// Every resource a net has ever held, in flat arrays indexed by the space's resource slots.
///
/// An FPGA graph can have hundreds of millions of resources, and a congested run touches
/// nearly all of them, so a hash lookup for every step of every search is what this avoids.
///
/// Storage only grows when a net occupies a resource, up to that resource's slot, so a run
/// that stays low in the slot numbers pays only for those. Nothing here asks the space how
/// many resources it has, which a space generated on demand could not say.
///
/// Reading takes `&self` and writing happens only between searches, so that searches can
/// share the table.
struct ResourceTable<S: RoutingSpace> {
    slots: S::ResourceSlots,
    /// Indexed by slot, and as long as the highest slot occupied so far requires.
    states: Vec<ResourceState>,
    /// One bit per slot, set if the slot is in `occupied_slots`.
    listed: Vec<u64>,
    /// The slots of every resource ever occupied. Going through these rather than `states`
    /// keeps the work per iteration in proportion to what the run has touched.
    occupied_slots: Vec<usize>,
    /// The resource in each of `occupied_slots`, since a space's mapping only goes one way.
    occupied_resources: Vec<S::Resource>,
    /// How many fixed routes hold each slot, as long as the highest slot one holds. Empty in
    /// a run without them, which is then exactly as it was before they existed. Kept apart
    /// from `states` so that pricing, which never reads it, has no more to load.
    fixed: Vec<u32>,
}

impl<S: RoutingSpace> ResourceTable<S> {
    fn new(space: &S) -> Self {
        ResourceTable {
            slots: space.new_resource_slots(),
            states: Vec::new(),
            listed: Vec::new(),
            occupied_slots: Vec::new(),
            occupied_resources: Vec::new(),
            fixed: Vec::new(),
        }
    }

    /// How many fixed routes hold `slot`.
    #[inline]
    fn fixed_at(&self, slot: usize) -> u32 {
        self.fixed.get(slot).copied().unwrap_or(0)
    }

    /// How far beyond capacity `slot` is through nets that could leave it: the overuse that
    /// is left once fixed routes are allowed whatever they take.
    #[inline]
    fn avoidable_overuse(&self, slot: usize) -> u32 {
        let state = self.states[slot];
        let allowed = state.capacity.max(self.fixed_at(slot));
        state.occupancy.saturating_sub(allowed)
    }

    /// Records that a fixed route holds each of `slots`, or with `add` false that it no
    /// longer does.
    fn mark_fixed(&mut self, slots: &[usize], add: bool) {
        for &slot in slots {
            if slot >= self.fixed.len() {
                self.fixed.resize(slot + 1, 0);
            }
            let count = &mut self.fixed[slot];
            *count = if add {
                count.saturating_add(1)
            } else {
                count.saturating_sub(1)
            };
        }
    }

    /// Returns the slot for `resource`, making room for it if needed.
    fn intern(&mut self, space: &S, resource: S::Resource) -> usize {
        let slot = self.slots.slot(resource);
        if slot >= self.states.len() {
            self.states.resize(slot + 1, ResourceState::UNOCCUPIED);
        }
        let (word, bit) = (slot / u64::BITS as usize, slot % u64::BITS as usize);
        if word >= self.listed.len() {
            self.listed.resize(word + 1, 0);
        }
        if self.listed[word] & (1 << bit) == 0 {
            self.listed[word] |= 1 << bit;
            self.occupied_slots.push(slot);
            self.occupied_resources.push(resource);
            self.states[slot].capacity = space.capacity(resource);
        }
        slot
    }

    /// Returns the state of `resource`, or `None` if it is free and has no history, in which
    /// case the caller has to ask the space for its capacity.
    ///
    /// Testing for that here is what makes the slots in between safe to read. An array entry
    /// for a slot that no net has occupied exists only because a higher slot was occupied, and
    /// its capacity of 0 means nothing. A real zero capacity has to be priced as one, so it
    /// must not be read from there.
    #[inline]
    fn read(&self, resource: S::Resource) -> Option<ResourceState> {
        let state = *self.states.get(self.slots.get(resource)?)?;
        (state.occupancy > 0 || state.history != Cost::ZERO).then_some(state)
    }

    /// Adds a net to each of `slots`.
    fn acquire(&mut self, slots: &[usize]) {
        for &slot in slots {
            let state = &mut self.states[slot];
            state.occupancy = state.occupancy.saturating_add(1);
        }
    }

    /// Removes a net from each of `slots`.
    fn release(&mut self, slots: &[usize]) {
        for &slot in slots {
            let state = &mut self.states[slot];
            state.occupancy = state.occupancy.saturating_sub(1);
        }
    }

    /// Returns how many resources are overused, by how much in total, and how much of that
    /// total fixed routes cause among themselves.
    fn overuse(&self) -> (u32, u64, u64) {
        let mut resources = 0;
        let mut total = 0;
        let mut fixed = 0;
        for &slot in &self.occupied_slots {
            let state = self.states[slot];
            let overuse = state.overuse();
            if overuse > 0 {
                resources += 1;
                total += u64::from(overuse);
                fixed += u64::from(self.fixed_at(slot).saturating_sub(state.capacity));
            }
        }
        (resources, total, fixed)
    }

    /// Charges every overused resource for the iteration just finished, by the overuse that
    /// nets could have avoided. That is all of it without fixed routes.
    fn add_history(&mut self, factor: Cost) {
        for index in 0..self.occupied_slots.len() {
            let slot = self.occupied_slots[index];
            let overuse = self.avoidable_overuse(slot);
            if overuse > 0 {
                let state = &mut self.states[slot];
                state.history = state.history + factor * Cost::from(overuse);
            }
        }
    }
}

/// A space whose steps cost what the negotiation currently says they do.
///
/// Everything else goes straight to the space inside. This exists for one search at a time,
/// borrowing the table, so it is created cheaply and only ever reads shared state.
struct Priced<'a, S: RoutingSpace> {
    inner: &'a S,
    table: &'a ResourceTable<S>,
    present: Cost,
    /// What the current net already holds. Using these again takes nothing from anyone.
    held: &'a FastSet<S::Resource>,
    /// A buffer for the space's answers. It is here because expansion only gets `&self`, and
    /// it is only ever borrowed within a single call.
    buffer: &'a RefCell<Vec<S::Resource>>,
    /// The route this net is replacing, when it is routed in a batch. The table still counts
    /// it, since other searches are reading the table, and a net must not compete with itself.
    replacing: &'a FastSet<S::Resource>,
    /// Room for working out which nodes a step ties in, for a space with [non-configurable
    /// steps](self#non-configurable-steps), and otherwise `None`.
    tied: Option<&'a RefCell<TiedScratch<S::Node>>>,
}

/// Buffers for [`tied_closure`].
struct TiedScratch<N> {
    /// The non-configurable steps found, as `(from, to)`.
    edges: Vec<(N, N)>,
    /// One answer of [`RoutingSpace::non_configurable`].
    found: Vec<N>,
}

impl<N> TiedScratch<N> {
    fn new() -> Self {
        TiedScratch {
            edges: Vec::new(),
            found: Vec::new(),
        }
    }
}

/// Puts in `scratch.edges`, as `(from, to)`, the non-configurable steps that reach every node
/// tied to `start`, through any number of them, each node once and in the order found. `came
/// from`, the node a step to `start` comes from, counts as reached already.
///
/// Sets of tied nodes are small, so a scan stands in for a set.
fn tied_closure<S: RoutingSpace>(
    space: &S,
    net: NetId,
    start: S::Node,
    came_from: Option<S::Node>,
    scratch: &mut TiedScratch<S::Node>,
) {
    let TiedScratch { edges, found } = scratch;
    edges.clear();
    let mut from = start;
    let mut next = 0;
    loop {
        found.clear();
        space.non_configurable(net, from, found);
        for &to in found.iter() {
            let reached = to == start
                || came_from == Some(to)
                || edges.iter().any(|&(_, already)| already == to);
            if !reached {
                edges.push((from, to));
            }
        }
        let Some(&(_, after)) = edges.get(next) else {
            break;
        };
        from = after;
        next += 1;
    }
}

impl<S: RoutingSpace> Priced<'_, S> {
    /// Adds the resources of everything tied to `to`, entered from `from`, to `resources`.
    #[inline]
    fn add_tied(
        &self,
        net: NetId,
        from: Option<S::Node>,
        to: S::Node,
        resources: &mut Vec<S::Resource>,
    ) {
        let Some(tied) = self.tied else {
            return;
        };
        let mut tied = tied.borrow_mut();
        tied_closure(self.inner, net, to, from, &mut tied);
        for &(a, b) in &tied.edges {
            self.inner.resources(net, a, b, resources);
        }
    }

    /// Applies the module's price formula to a step that consumes `resources`.
    #[inline]
    fn price(&self, base: Cost, resources: &mut Vec<S::Resource>) -> Cost {
        // The sums below must not count a resource twice or depend on the order a space
        // lists resources in. Nearly every step has one, so this is rarely any work.
        if resources.len() > 1 {
            resources.sort_unstable();
            resources.dedup();
        }

        let mut history = Cost::ZERO;
        let mut overuse = 0u32;
        for &resource in resources.iter() {
            if !self.held.is_empty() && self.held.contains(&resource) {
                continue;
            }
            let here = match self.table.read(resource) {
                Some(state) => {
                    history = history + state.history;
                    // Occupied comes first because free resources are the common case, and
                    // they spare a lookup on a set that is usually not empty in a batch.
                    let others = if state.occupancy > 0 && self.replacing.contains(&resource) {
                        state.occupancy - 1
                    } else {
                        state.occupancy
                    };
                    overuse_with_one_more(others, state.capacity)
                }
                // Free, and yet it can still be overused by a first net if its capacity is
                // zero, and a search must see that up front. Otherwise each iteration would
                // discover only those it blundered into, and a problem with many would run
                // out of iterations.
                None => overuse_with_one_more(0, self.inner.capacity(resource)),
            };
            overuse = overuse.saturating_add(here);
        }

        if history == Cost::ZERO && overuse == 0 {
            return base;
        }
        (base + history) * (Cost::ONE + self.present * Cost::from(overuse))
    }

    /// Returns what it costs to start a tree at `node`, given its access cost.
    fn source_cost(&self, net: NetId, node: S::Node, access: Cost) -> Cost {
        let mut buffer = self.buffer.borrow_mut();
        buffer.clear();
        self.inner.resources(net, node, node, &mut buffer);
        self.add_tied(net, None, node, &mut buffer);
        self.price(access, &mut buffer)
    }
}

impl<S: RoutingSpace> RoutingSpace for Priced<'_, S> {
    type Node = S::Node;
    type Resource = S::Resource;
    type Slots = S::Slots;
    type ResourceSlots = S::ResourceSlots;

    fn new_slots(&self) -> S::Slots {
        self.inner.new_slots()
    }

    fn new_resource_slots(&self) -> S::ResourceSlots {
        self.inner.new_resource_slots()
    }

    #[inline]
    fn contains(&self, node: S::Node) -> bool {
        self.inner.contains(node)
    }

    #[inline]
    fn expand(&self, net: NetId, from: S::Node, out: &mut Vec<Step<S::Node>>) {
        let start = out.len();
        self.inner.expand(net, from, out);
        let mut buffer = self.buffer.borrow_mut();
        for step in &mut out[start..] {
            buffer.clear();
            self.inner.resources(net, from, step.to, &mut buffer);
            self.add_tied(net, Some(from), step.to, &mut buffer);
            step.cost = self.price(step.cost, &mut buffer);
        }
    }

    #[inline]
    fn lower_bound(&self, net: NetId, from: S::Node, to: S::Node) -> Cost {
        self.inner.lower_bound(net, from, to)
    }

    fn resources(&self, net: NetId, from: S::Node, to: S::Node, out: &mut Vec<S::Resource>) {
        self.inner.resources(net, from, to, out);
    }

    fn capacity(&self, resource: S::Resource) -> u32 {
        self.inner.capacity(resource)
    }

    fn non_configurable(&self, net: NetId, from: S::Node, out: &mut Vec<S::Node>) {
        self.inner.non_configurable(net, from, out);
    }

    fn has_non_configurable(&self) -> bool {
        self.inner.has_non_configurable()
    }
}

/// [`DelaySpace::step_delays`] for the space of an engine, which only a timing-driven engine
/// has. Keeping it as a pointer is what spares every other engine a `DelaySpace` bound.
type StepDelays<S> = fn(&S, NetId, <S as RoutingSpace>::Node, &mut Vec<u32>);
/// [`DelaySpace::delay_lower_bound`], in the same way.
type DelayLowerBound<S> =
    fn(&S, NetId, <S as RoutingSpace>::Node, <S as RoutingSpace>::Node) -> u32;

/// What shrinks the blended lower bound, so that rounding cannot lift it above the sum of the
/// rounded step costs it bounds. Each blended step and the bound itself are a few roundings
/// of relative size 2^-53 from exact, and this is a thousand times that.
const BLENDED_BOUND_SHRINK: f64 = 1.0 - 1e-12;

/// A priced space as one connection of a given criticality `c` sees it, after VPR: a step
/// costs `(1 - c) * price + c * delay_cost * delay`.
///
/// Its lower bound is the same blend of the space's two lower bounds, which is admissible
/// because each of them is. A price is never below the step's base cost, which the space's
/// cost bound is under, and the delays it adds up are the ones its delay bound is under. Both
/// weights are not negative, so the blend of the bounds is under the blend of the sums.
/// Scaling matters here: the cost bound alone, unscaled, would overestimate a step that costs
/// more than its delay is worth.
struct Blended<'p, 'a, S: RoutingSpace> {
    priced: &'p Priced<'a, S>,
    step_delays: StepDelays<S>,
    delay_bound: DelayLowerBound<S>,
    /// `1 - c`.
    congestion: Cost,
    /// `c` times the cost of a picosecond.
    delay: Cost,
    /// For the space's answers, borrowed only within a call.
    delays: &'p RefCell<Vec<u32>>,
}

impl<S: RoutingSpace> RoutingSpace for Blended<'_, '_, S> {
    type Node = S::Node;
    type Resource = S::Resource;
    type Slots = S::Slots;
    type ResourceSlots = S::ResourceSlots;

    fn new_slots(&self) -> S::Slots {
        self.priced.new_slots()
    }

    fn new_resource_slots(&self) -> S::ResourceSlots {
        self.priced.new_resource_slots()
    }

    #[inline]
    fn contains(&self, node: S::Node) -> bool {
        self.priced.contains(node)
    }

    #[inline]
    fn expand(&self, net: NetId, from: S::Node, out: &mut Vec<Step<S::Node>>) {
        let start = out.len();
        self.priced.expand(net, from, out);
        let mut delays = self.delays.borrow_mut();
        delays.clear();
        (self.step_delays)(self.priced.inner, net, from, &mut delays);
        for (position, step) in out[start..].iter_mut().enumerate() {
            // A space that gives too few delays breaks its contract, and gets no delay for
            // the steps it left out rather than a panic.
            let delay = delays.get(position).copied().unwrap_or(0);
            step.cost = self.congestion * step.cost + self.delay * Cost::from(delay);
        }
    }

    #[inline]
    fn lower_bound(&self, net: NetId, from: S::Node, to: S::Node) -> Cost {
        let cost = self.priced.inner.lower_bound(net, from, to);
        let delay = (self.delay_bound)(self.priced.inner, net, from, to);
        let blended = self.congestion * cost + self.delay * Cost::from(delay);
        Cost::new(blended.get() * BLENDED_BOUND_SHRINK).unwrap_or(Cost::ZERO)
    }

    fn resources(&self, net: NetId, from: S::Node, to: S::Node, out: &mut Vec<S::Resource>) {
        self.priced.resources(net, from, to, out);
    }

    fn capacity(&self, resource: S::Resource) -> u32 {
        self.priced.capacity(resource)
    }

    fn non_configurable(&self, net: NetId, from: S::Node, out: &mut Vec<S::Node>) {
        self.priced.non_configurable(net, from, out);
    }

    fn has_non_configurable(&self) -> bool {
        self.priced.has_non_configurable()
    }
}

/// Why a net's routing stopped early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interrupt {
    Budget,
    Cancel,
}

impl Interrupt {
    fn reason(self) -> UnroutedReason {
        match self {
            Interrupt::Budget => UnroutedReason::BudgetExhausted,
            Interrupt::Cancel => UnroutedReason::Cancelled,
        }
    }

    fn stop(self) -> StopReason {
        match self {
            Interrupt::Budget => StopReason::BudgetExhausted,
            Interrupt::Cancel => StopReason::Cancelled,
        }
    }
}

/// The result of routing one net, not yet committed to the resource table.
enum Attempt<N, R> {
    Routed {
        route: Route<N>,
        cost: Cost,
        /// Everything the route consumes, in ascending order and without repeats.
        resources: Vec<R>,
        /// The delay to each terminal, by index, when timing-driven, and otherwise empty.
        delays: Vec<u32>,
    },
    NoPath,
    Interrupted(Interrupt),
}

/// Everything one thread needs in order to route nets. Nothing here outlives a net except
/// allocations and counters.
struct Worker<S: RoutingSpace> {
    search: Search<S>,
    buffer: RefCell<Vec<S::Resource>>,
    /// What the tree being grown holds, for quick membership tests.
    held: FastSet<S::Resource>,
    /// The same in the order first held, because the set must not be iterated.
    held_list: Vec<S::Resource>,
    sources: Vec<Source<S::Node>>,
    targets: Vec<Target<S::Node>>,
    /// Indices of the terminals still to connect.
    pending: Vec<usize>,
    path: Vec<S::Node>,
    /// The nodes new to the tree after a path, for finding the terminals it connected.
    new_nodes: FastSet<S::Node>,
    steps: Vec<Step<S::Node>>,
    /// The route being replaced, when routing in a batch.
    replacing: FastSet<S::Resource>,
    /// A buffer for a space's delays, borrowed only within a single call.
    delay_buffer: RefCell<Vec<u32>>,
    /// The delay from the root to each node of the tree, when timing-driven. Only looked up.
    arrival: FastMap<S::Node, u32>,
    /// The delay to each terminal, by index, when timing-driven.
    terminal_delays: Vec<u32>,
    /// For working out what is tied to a node.
    tied: RefCell<TiedScratch<S::Node>>,
    /// The nodes that the most recent path tied in, besides its own.
    tied_added: Vec<S::Node>,
    searches: u64,
    expanded: u64,
}

impl<S: RoutingSpace> Worker<S> {
    fn new(space: &S) -> Self {
        Worker {
            search: Search::new(space),
            buffer: RefCell::new(Vec::new()),
            held: FastSet::default(),
            held_list: Vec::new(),
            sources: Vec::new(),
            targets: Vec::new(),
            pending: Vec::new(),
            path: Vec::new(),
            new_nodes: FastSet::default(),
            steps: Vec::new(),
            replacing: FastSet::default(),
            delay_buffer: RefCell::new(Vec::new()),
            arrival: FastMap::default(),
            terminal_delays: Vec::new(),
            tied: RefCell::new(TiedScratch::new()),
            tied_added: Vec::new(),
            searches: 0,
            expanded: 0,
        }
    }

    /// Records what `net` consumes by stepping from `from` to `to`.
    fn hold(&mut self, space: &S, net: NetId, from: S::Node, to: S::Node) {
        let buffer = self.buffer.get_mut();
        buffer.clear();
        space.resources(net, from, to, buffer);
        for &resource in buffer.iter() {
            if self.held.insert(resource) {
                self.held_list.push(resource);
            }
        }
    }

    /// Adds the edges from the most recent path to the tree, and returns what they added to
    /// its cost and how far along the path they start. With `step_delays`, this also works
    /// out the delay from the root to each new node.
    fn adopt_path(
        &mut self,
        space: &S,
        net: NetId,
        route: &mut Route<S::Node>,
        step_delays: Option<StepDelays<S>>,
    ) -> (Cost, usize) {
        let starting = route.is_empty();
        // Sources are the tree itself, so a path only leaves the tree, and has no repeats
        // because each node of it has a single parent. A failure would mean a broken
        // `NodeSlots` that gives two nodes one slot.
        let added = route
            .add_path(&self.path)
            .expect("a search path runs from the tree without repeating a node");
        let from_index = self.path.len() - 1 - added;

        if starting {
            let root = self.path[0];
            self.hold(space, net, root, root);
            if step_delays.is_some() {
                self.arrival.insert(root, 0);
            }
        }
        let mut cost = Cost::ZERO;
        for index in from_index..self.path.len() - 1 {
            let (from, to) = (self.path[index], self.path[index + 1]);
            self.steps.clear();
            space.expand(net, from, &mut self.steps);
            // Searches do not report the base cost, because they only saw the price. Paths
            // are a tiny part of what a search expands, so asking again costs little.
            let position = self.steps.iter().position(|step| step.to == to);
            if let Some(position) = position {
                cost = cost + self.steps[position].cost;
            }
            if let Some(step_delays) = step_delays {
                let delays = self.delay_buffer.get_mut();
                delays.clear();
                step_delays(space, net, from, delays);
                let delay = position.and_then(|p| delays.get(p).copied()).unwrap_or(0);
                let before = self.arrival.get(&from).copied().unwrap_or(0);
                self.arrival.insert(to, before.saturating_add(delay));
            }
            self.hold(space, net, from, to);
        }

        self.tied_added.clear();
        if space.has_non_configurable() {
            let first = if starting { 0 } else { from_index + 1 };
            for index in first..self.path.len() {
                let node = self.path[index];
                cost = cost + self.add_tied(space, net, route, node, step_delays);
            }
        }
        (cost, from_index)
    }

    /// Adds to the tree every node tied to `node` that it does not have yet, through the
    /// non-configurable steps that reach them, and returns their cost.
    fn add_tied(
        &mut self,
        space: &S,
        net: NetId,
        route: &mut Route<S::Node>,
        node: S::Node,
        step_delays: Option<StepDelays<S>>,
    ) -> Cost {
        let mut tied = core::mem::replace(self.tied.get_mut(), TiedScratch::new());
        tied_closure(space, net, node, None, &mut tied);
        let mut cost = Cost::ZERO;
        for &(from, to) in &tied.edges {
            // On the tree already, by the path or by another node's ties.
            if route.contains(to) {
                continue;
            }
            route
                .add_path(&[from, to])
                .expect("a tied node is found from one on the tree");
            self.steps.clear();
            space.expand(net, from, &mut self.steps);
            let position = self.steps.iter().position(|step| step.to == to);
            if let Some(position) = position {
                cost = cost + self.steps[position].cost;
            }
            if let Some(step_delays) = step_delays {
                let delays = self.delay_buffer.get_mut();
                delays.clear();
                step_delays(space, net, from, delays);
                let delay = position.and_then(|p| delays.get(p).copied()).unwrap_or(0);
                let before = self.arrival.get(&from).copied().unwrap_or(0);
                self.arrival.insert(to, before.saturating_add(delay));
            }
            self.hold(space, net, from, to);
            self.tied_added.push(to);
        }
        *self.tied.get_mut() = tied;
        cost
    }

    /// Takes the terminals connected by the most recent path off the list, and returns
    /// the sum of their access costs. When timing-driven, this records their delays.
    fn connect_terminals(&mut self, net: &Net<S::Node>, from_index: usize, timed: bool) -> Cost {
        self.new_nodes.clear();
        self.new_nodes.extend(&self.path[from_index..]);
        self.new_nodes.extend(&self.tied_added);
        let new_nodes = &self.new_nodes;
        let arrival = &self.arrival;
        let terminal_delays = &mut self.terminal_delays;
        let mut cost = Cost::ZERO;
        self.pending.retain(|&terminal| {
            let points = net.terminals[terminal].access_points();
            let on_path = || {
                points
                    .iter()
                    .filter(|point| new_nodes.contains(&point.node))
            };
            let Some(access) = on_path().map(|point| point.cost).min() else {
                return true;
            };
            cost = cost + access;
            if timed {
                // A sink with several access points on the tree is as fast as the fastest.
                let delay = on_path()
                    .filter_map(|point| arrival.get(&point.node).copied())
                    .min();
                terminal_delays[terminal] = delay.unwrap_or(0);
            }
            false
        });
        cost
    }
}

/// What a timing-driven engine tells each net while it is being routed: how to learn delays,
/// and how critical its connections are. None of it changes during an iteration.
struct RouteTiming<'a, S: RoutingSpace> {
    step_delays: StepDelays<S>,
    delay_bound: DelayLowerBound<S>,
    delay_cost: Cost,
    /// The criticality of every terminal of every net, at `offsets[net] + terminal`.
    criticality: &'a [f64],
    offsets: &'a [u32],
}

// By hand, because a derive would ask for `S: Copy` when only pointers to its methods are here.
impl<S: RoutingSpace> Clone for RouteTiming<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: RoutingSpace> Copy for RouteTiming<'_, S> {}

impl<'a, S: RoutingSpace> RouteTiming<'a, S> {
    /// Returns the criticality of each terminal of the net at `index`.
    fn of_net(&self, index: usize) -> &'a [f64] {
        let start = self.offsets[index] as usize;
        let end = self.offsets[index + 1] as usize;
        &self.criticality[start..end]
    }
}

/// What every net of an iteration sees, which nothing changes while a net is being routed.
struct RouteContext<'a, S: RoutingSpace> {
    space: &'a S,
    table: &'a ResourceTable<S>,
    present: Cost,
    cancel: Option<&'a AtomicBool>,
    timing: Option<RouteTiming<'a, S>>,
}

/// Returns the most critical of the terminals still to connect, if any has a criticality
/// above 0, with that criticality. The lowest index wins a tie, which keeps the order fixed.
fn most_critical(pending: &[usize], criticality: &[f64]) -> Option<(usize, f64)> {
    let mut best: Option<(usize, f64)> = None;
    for &terminal in pending {
        let here = criticality.get(terminal).copied().unwrap_or(0.0);
        if here > 0.0 && best.is_none_or(|(_, most)| here > most) {
            best = Some((terminal, here));
        }
    }
    best
}

/// Routes one net that needs it against the current state of the table.
///
/// `budget` is how many expansions it may use, and `dead_roots` lists the driver's access
/// points known not to reach every terminal. `criticality` has the criticality of each of the
/// net's terminals when the run is timing-driven. A net with a `fixed` part grows from it.
fn route_net<S: RoutingSpace>(
    context: &RouteContext<'_, S>,
    net: &Net<S::Node>,
    criticality: &[f64],
    budget: u64,
    dead_roots: &mut Vec<S::Node>,
    fixed: Option<&FixedPart<S::Node, S::Resource>>,
    worker: &mut Worker<S>,
) -> Attempt<S::Node, S::Resource> {
    let space = context.space;
    let timing = context.timing;
    let mut spent = 0u64;
    let Some(driver) = net.driver() else {
        return Attempt::NoPath;
    };

    // Each time around, one more root is known to be useless, so this ends.
    'roots: loop {
        worker.held.clear();
        worker.held_list.clear();
        worker.pending.clear();
        worker.arrival.clear();
        worker.terminal_delays.clear();
        if timing.is_some() {
            worker.terminal_delays.resize(net.terminals.len(), 0);
        }
        let mut route = Route::new();
        let mut cost = Cost::ZERO;
        match fixed {
            None => worker.pending.extend(1..net.terminals.len()),
            Some(fixed) => {
                route.clone_from(&fixed.route);
                cost = fixed.cost;
                worker.pending.extend_from_slice(&fixed.pending);
                worker.held.extend(fixed.resources.iter().copied());
                worker.held_list.extend_from_slice(&fixed.resources);
                worker.arrival.extend(fixed.arrivals.iter().copied());
                if timing.is_some() {
                    worker.terminal_delays.clone_from(&fixed.delays);
                }
            }
        }

        while !worker.pending.is_empty() {
            let priced = Priced {
                inner: space,
                table: context.table,
                present: context.present,
                held: &worker.held,
                buffer: &worker.buffer,
                replacing: &worker.replacing,
                tied: space.has_non_configurable().then_some(&worker.tied),
            };
            // A critical connection is routed on its own, most critical first, as in VPR, so
            // that it gets its own weight on delay and the first choice of the tree. Those of
            // no criticality are left to the end, and routed as without timing: each search
            // connects whichever is nearest, which makes the same trees as a timing-free run.
            let critical = timing.and_then(|timing| {
                let (terminal, c) = most_critical(&worker.pending, criticality)?;
                let blend = Blended {
                    priced: &priced,
                    step_delays: timing.step_delays,
                    delay_bound: timing.delay_bound,
                    congestion: Cost::new(1.0 - c).unwrap_or(Cost::ZERO),
                    delay: Cost::new(c).unwrap_or(Cost::ZERO) * timing.delay_cost,
                    delays: &worker.delay_buffer,
                };
                Some((terminal, blend))
            });

            worker.sources.clear();
            if route.is_empty() {
                for point in driver.access_points() {
                    // A node outside the space has no resources to be priced by. The search
                    // would ignore it anyway.
                    if !dead_roots.contains(&point.node) && space.contains(point.node) {
                        let start = priced.source_cost(net.id, point.node, point.cost);
                        let start = match &critical {
                            Some((_, blend)) => blend.congestion * start,
                            None => start,
                        };
                        worker.sources.push(Source::with_cost(point.node, start));
                    }
                }
            } else if let Some((_, blend)) = &critical {
                // A connection that joins the tree far from the root has the delay of getting
                // there to pay for, as VPR charges it.
                let arrival = &worker.arrival;
                worker.sources.extend(route.nodes().map(|node| {
                    let delay = arrival.get(&node).copied().unwrap_or(0);
                    Source::with_cost(node, blend.delay * Cost::from(delay))
                }));
            } else {
                worker.sources.extend(route.nodes().map(Source::new));
            }
            worker.targets.clear();
            match &critical {
                Some((terminal, blend)) => {
                    let points = net.terminals[*terminal].access_points();
                    let targets = points
                        .iter()
                        .map(|point| Target::with_cost(point.node, blend.congestion * point.cost));
                    worker.targets.extend(targets);
                }
                None => {
                    for &terminal in &worker.pending {
                        let points = net.terminals[terminal].access_points();
                        let targets = points
                            .iter()
                            .map(|point| Target::with_cost(point.node, point.cost));
                        worker.targets.extend(targets);
                    }
                }
            }

            let limits = SearchLimits {
                max_expansions: budget.saturating_sub(spent),
                cancel: context.cancel,
            };
            let sources = worker.sources.iter().copied();
            let targets = worker.targets.iter().copied();
            let outcome = match &critical {
                Some((_, blend)) => worker
                    .search
                    .find_path_with(blend, net.id, sources, targets, limits),
                None => worker
                    .search
                    .find_path_with(&priced, net.id, sources, targets, limits),
            };
            let end = match outcome {
                SearchOutcome::Found(found) => {
                    worker.path.clear();
                    worker.path.extend_from_slice(found.path);
                    Ok(true)
                }
                SearchOutcome::Unreachable => Ok(false),
                SearchOutcome::LimitReached => Err(Interrupt::Budget),
                SearchOutcome::Cancelled => Err(Interrupt::Cancel),
            };
            let expanded = worker.search.stats().expanded;
            spent = spent.saturating_add(expanded);
            worker.searches += 1;
            worker.expanded += expanded;

            match end {
                Ok(true) => {}
                Ok(false) => match route.root() {
                    // With no tree yet, every root that remains has been searched from, and a
                    // fixed part has the only root there can be.
                    None => return Attempt::NoPath,
                    Some(_) if fixed.is_some() => return Attempt::NoPath,
                    Some(root) => {
                        dead_roots.push(root);
                        continue 'roots;
                    }
                },
                Err(interrupt) => return Attempt::Interrupted(interrupt),
            }

            if route.is_empty() {
                let root = worker.path[0];
                let access = driver
                    .access_points()
                    .iter()
                    .filter(|point| point.node == root)
                    .map(|point| point.cost)
                    .min();
                cost = cost + access.unwrap_or(Cost::ZERO);
            }
            let step_delays = timing.map(|timing| timing.step_delays);
            let (steps_cost, from_index) =
                worker.adopt_path(space, net.id, &mut route, step_delays);
            cost = cost + steps_cost;
            cost = cost + worker.connect_terminals(net, from_index, timing.is_some());
        }

        let mut resources = worker.held_list.clone();
        resources.sort_unstable();
        return Attempt::Routed {
            route,
            cost,
            resources,
            delays: worker.terminal_delays.clone(),
        };
    }
}

/// A net routed in a batch: the table as it stood when the batch began decided this, so no
/// thread count or order of finishing can change it.
struct BatchRoute<N, R> {
    attempt: Attempt<N, R>,
    /// A copy, because the net's own list is shared with the other members of the batch. It
    /// is adopted along with the route, so that a route that is thrown away leaves no trace
    /// but its charge. Finding these again costs little, as they are rare.
    dead_roots: Vec<N>,
    searches: u64,
    expanded: u64,
    /// What the engine's count of commits stood at when the batch began.
    commits_seen: u64,
}

/// Routes the net at `index` as a member of a batch, leaving the table to the caller.
fn route_in_batch<S: RoutingSpace>(
    context: &BatchContext<'_, S>,
    index: u32,
    worker: &mut Worker<S>,
) -> BatchRoute<S::Node, S::Resource> {
    let index = index as usize;
    let state = &context.states[index];
    worker.replacing.clear();
    worker.replacing.extend(state.resources.iter().copied());
    let mut dead_roots = state.dead_roots.clone();
    let criticality = context.route.timing.map_or(&[][..], |t| t.of_net(index));
    let attempt = route_net(
        &context.route,
        &context.nets[index],
        criticality,
        context.budget,
        &mut dead_roots,
        state.fixed.as_deref(),
        worker,
    );
    worker.replacing.clear();
    let (searches, expanded) = (worker.searches, worker.expanded);
    worker.searches = 0;
    worker.expanded = 0;
    BatchRoute {
        attempt,
        dead_roots,
        searches,
        expanded,
        commits_seen: context.commits,
    }
}

/// The routes of a batch, each with the index of its net.
type RoutedBatch<S> = Vec<(
    u32,
    BatchRoute<<S as RoutingSpace>::Node, <S as RoutingSpace>::Resource>,
)>;

/// What every member of a batch reads, none of which changes until the batch has finished.
struct BatchContext<'a, S: RoutingSpace> {
    route: RouteContext<'a, S>,
    nets: &'a [Net<S::Node>],
    states: &'a [NetState<S::Node, S::Resource>],
    budget: u64,
    commits: u64,
}

/// Where one net stands.
struct NetState<N, R> {
    /// The current route, which is `None` until the net has been routed.
    route: Option<Route<N>>,
    /// The table slots the route holds.
    held: Vec<usize>,
    /// The resources in those slots, in ascending order.
    resources: Vec<R>,
    cost: Cost,
    /// The delay to each terminal of the route, by index, when timing-driven.
    delays: Vec<u32>,
    /// Access points of the driver known not to reach every terminal. Reachability does not
    /// depend on prices, so this is true for the rest of the run.
    dead_roots: Vec<N>,
    /// Whether the net can never be routed, because no root reaches every terminal or
    /// because a terminal is entirely outside the space.
    no_path: bool,
    /// The part of the route that the caller fixed, if any.
    fixed: Option<Box<FixedPart<N, R>>>,
}

impl<N, R> NetState<N, R> {
    /// Whether the whole route is fixed, so that the net is never routed.
    fn is_fixed(&self) -> bool {
        self.fixed.as_ref().is_some_and(|fixed| fixed.complete)
    }
}

/// A route given by the caller, and what the engine worked out from it once, at the start.
struct FixedPart<N, R> {
    route: Route<N>,
    /// Everything it consumes, in ascending order and without repeats.
    resources: Vec<R>,
    /// The table slots of those, in ascending order, for lookups.
    slots: Vec<usize>,
    /// Its cost, as [`Outcome::total_cost`] counts it.
    cost: Cost,
    /// Whether it reaches every terminal.
    complete: bool,
    /// The terminals it does not reach, in ascending order.
    pending: Vec<usize>,
    /// The delay from the root to each of its nodes, when timing-driven.
    arrivals: Vec<(N, u32)>,
    /// The delay to each terminal it reaches, by index, when timing-driven, and 0 for the
    /// others.
    delays: Vec<u32>,
}

/// What came of giving every net its turn.
#[derive(Default)]
struct Turns {
    nets_rerouted: u32,
    interrupted: Option<Interrupt>,
    batches: u32,
    discarded: u32,
}

/// The best iteration seen.
struct Best<N, R> {
    routes: RouteSet<N>,
    overused: Vec<OverusedResource<R>>,
    iteration: u32,
    rank: Rank,
    total_cost: Cost,
    timing: Option<TimingOutcome>,
}

/// What decides which iteration is best, compared in this order, where less is better.
///
/// Timing comes after legality and before cost, as in VPR's `is_better_quality_routing`,
/// which ranks legal routings by worst negative slack, then total negative slack, then
/// wirelength. Here every iteration is ranked, legal or not, so legality comes first. Without
/// timing, both slacks are 0 and the order is as it always was.
///
/// A run that [improves timing](EngineOptions::timing_iterations) after its first legal result
/// also ranks by critical-path delay after the slacks, so that a shorter critical path counts
/// once timing is met. Otherwise that is 0, which keeps the order as it was.
type Rank = (u32, u64, i64, i128, u64, Cost);

/// The rank of an iteration with the given state.
fn rank(
    unrouted: u32,
    total_overuse: u64,
    timing: Option<&TimingMetrics>,
    by_delay: bool,
    cost: Cost,
) -> Rank {
    let (worst, total) = timing.map_or((0, 0), |metrics| {
        (
            metrics.worst_negative_slack(),
            metrics.total_negative_slack.min(0),
        )
    });
    let delay = timing
        .filter(|_| by_delay)
        .and_then(|metrics| metrics.critical_path_delay)
        .unwrap_or(0);
    // Negated so that less is better, as for the rest.
    (
        unrouted,
        total_overuse,
        worst.saturating_neg(),
        total.saturating_neg(),
        delay,
        cost,
    )
}

/// Returns the delay that the current route of the net at `index` gives one of its terminals,
/// if it has a route.
fn delay_of<N, R>(states: &[NetState<N, R>], index: usize, terminal: usize) -> Option<u32> {
    let state = &states[index];
    state.route.as_ref()?;
    Some(state.delays.get(terminal).copied().unwrap_or(0))
}

/// The timing side of a timing-driven engine.
struct TimingState<S: RoutingSpace> {
    step_delays: StepDelays<S>,
    delay_bound: DelayLowerBound<S>,
    delay_cost: Cost,
    graph: TimingGraph,
    analysis: TimingAnalysis,
    constraints: Constraints,
    options: CriticalityOptions,
    /// As given, for the order of [`TimingOutcome::connection_delays`].
    connections: Vec<Connection>,
    /// Where each connection is: its net's index, its terminal, and its slot, which is at
    /// `offsets[net] + terminal`.
    places: Vec<(usize, usize, usize)>,
    /// Where each net's terminals start among the slots.
    offsets: Vec<u32>,
    /// The criticality of every terminal slot, which only changes between iterations, so that
    /// all of an iteration's nets see the same ones whatever the batches and threads.
    criticality: Vec<f64>,
    /// The delay of every arc, by [`ArcId`], for the timing graph to take its net delays from.
    arc_delays: Vec<u32>,
    /// A buffer for the analysis's criticalities, by [`ArcId`].
    arc_criticality: Vec<f64>,
    /// The criticality from which a net counts as critical.
    critical_from: f64,
    /// For each net, whether a connection of it is at least `critical_from`.
    critical_nets: Vec<bool>,
    metrics: TimingMetrics,
}

impl<S: RoutingSpace> TimingState<S> {
    /// Checks the connections and runs a first analysis on the estimated delays.
    fn new<N>(
        nets: &[Net<N>],
        timing: TimingDriven,
        step_delays: StepDelays<S>,
        delay_bound: DelayLowerBound<S>,
        critical_from: f64,
    ) -> Result<Self, EngineError> {
        let TimingDriven {
            graph,
            constraints,
            connections,
            criticality: options,
            delay_cost,
        } = timing;

        let mut offsets = Vec::with_capacity(nets.len() + 1);
        let mut total = 0u32;
        for net in nets {
            offsets.push(total);
            total = total.saturating_add(net.terminals.len() as u32);
        }
        offsets.push(total);
        // Looked up and never iterated.
        let index_of: FastMap<NetId, usize> = nets
            .iter()
            .enumerate()
            .map(|(index, net)| (net.id, index))
            .collect();

        let mut places = Vec::with_capacity(connections.len());
        let mut slot_taken = vec![false; total as usize];
        let mut arc_taken = vec![false; graph.num_arcs() as usize];
        for connection in &connections {
            let Connection { net, terminal, arc } = *connection;
            let index = *index_of.get(&net).ok_or(EngineError::UnknownNet { net })?;
            if terminal == 0 || terminal as usize >= nets[index].terminals.len() {
                return Err(EngineError::NotASink { net, terminal });
            }
            if graph.arc(arc).is_none_or(|info| info.kind != ArcKind::Net) {
                return Err(EngineError::NotANetArc { arc });
            }
            let slot = offsets[index] as usize + terminal as usize;
            if slot_taken[slot] || arc_taken[arc.index()] {
                return Err(EngineError::DuplicateConnection { net, terminal });
            }
            slot_taken[slot] = true;
            arc_taken[arc.index()] = true;
            places.push((index, terminal as usize, slot));
        }

        let arc_delays = (0..graph.num_arcs())
            .map(|arc| graph.arc(ArcId(arc)).map_or(0, |info| info.delay))
            .collect();
        let analysis = TimingAnalysis::new(&graph);
        let mut state = TimingState {
            step_delays,
            delay_bound,
            delay_cost,
            graph,
            analysis,
            constraints,
            options,
            connections,
            places,
            offsets,
            criticality: vec![0.0; total as usize],
            arc_delays,
            arc_criticality: Vec::new(),
            critical_from,
            critical_nets: vec![false; nets.len()],
            metrics: TimingMetrics {
                critical_path_delay: None,
                worst_slack: None,
                total_negative_slack: 0,
            },
        };
        state.run();
        state.refresh_criticality();
        Ok(state)
    }

    /// Runs the analysis on the delays the graph has.
    fn run(&mut self) {
        self.analysis.run(&self.graph, &self.constraints);
        self.metrics = TimingMetrics {
            critical_path_delay: self.analysis.critical_path().map(|path| path.arrival()),
            worst_slack: self.analysis.worst_slack(),
            total_negative_slack: self.analysis.total_negative_slack(),
        };
    }

    /// Takes the delays of the routes there are, and analyses them. A connection whose net has
    /// no route keeps the delay it had.
    fn analyse<R>(&mut self, states: &[NetState<S::Node, R>]) {
        for (connection, &(net, terminal, _)) in self.connections.iter().zip(&self.places) {
            if let Some(delay) = delay_of(states, net, terminal) {
                self.arc_delays[connection.arc.index()] = delay;
            }
        }
        let delays = &self.arc_delays;
        self.graph
            .apply_net_delays(&|arc: ArcId| delays.get(arc.index()).copied().unwrap_or(0));
        self.run();
    }

    /// Sets the criticality of every connection from the latest analysis.
    fn refresh_criticality(&mut self) {
        self.analysis
            .fill_arc_criticalities(&self.options, &mut self.arc_criticality);
        self.critical_nets.fill(false);
        for (connection, &(net, _, slot)) in self.connections.iter().zip(&self.places) {
            let criticality = self.arc_criticality[connection.arc.index()];
            self.criticality[slot] = criticality;
            if criticality >= self.critical_from {
                self.critical_nets[net] = true;
            }
        }
    }

    /// Whether any net counts as critical.
    fn any_critical(&self) -> bool {
        self.critical_nets.contains(&true)
    }

    /// Returns the timing of the current routes, as last analysed.
    fn outcome<R>(&self, states: &[NetState<S::Node, R>]) -> TimingOutcome {
        TimingOutcome {
            metrics: self.metrics,
            connection_delays: self
                .places
                .iter()
                .map(|&(net, terminal, _)| delay_of(states, net, terminal))
                .collect(),
        }
    }

    /// Returns what the nets of an iteration need to know.
    fn route_timing(&self) -> RouteTiming<'_, S> {
        RouteTiming {
            step_delays: self.step_delays,
            delay_bound: self.delay_bound,
            delay_cost: self.delay_cost,
            criticality: &self.criticality,
            offsets: &self.offsets,
        }
    }
}

/// A negotiated-congestion router, as a state machine.
///
/// Each call to [`step`](Engine::step) or [`step_parallel`](Engine::step_parallel) runs one
/// iteration and reports on it. A caller can stop stepping whenever it likes and take the best
/// result so far from [`outcome`](Engine::outcome). See the [module documentation](self) for
/// how it works.
///
/// # Examples
///
/// ```
/// use guide::engine::{Engine, EngineOptions};
/// use guide::id::NetId;
/// use guide::net::{Net, Terminal};
/// use guide::space::grid::Grid;
///
/// let grid = Grid::new(5, 5, 2)?;
/// let at = |x, y| Terminal::at(grid.node_at(x, y, 0).unwrap());
/// let nets = [
///     Net { id: NetId(0), terminals: vec![at(0, 2), at(4, 2)] },
///     Net { id: NetId(1), terminals: vec![at(2, 0), at(2, 4)] },
/// ];
///
/// let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
///
/// // Both nets go straight at first, and meet in the middle.
/// let first = engine.step().unwrap();
/// assert_eq!(first.total_overuse, 1);
/// assert_eq!(engine.overused()[0].nets, [NetId(0), NetId(1)]);
/// assert!(!engine.outcome().is_success());
///
/// while let Some(report) = engine.step() {
///     println!("iteration {}: overuse {}", report.iteration, report.total_overuse);
/// }
/// assert!(engine.outcome().is_success());
/// # Ok::<(), guide::space::grid::GridError>(())
/// ```
pub struct Engine<'a, S: RoutingSpace> {
    space: &'a S,
    nets: &'a [Net<S::Node>],
    options: EngineOptions<'a>,
    table: ResourceTable<S>,
    states: Vec<NetState<S::Node, S::Resource>>,
    /// One for each thread so far, and always at least one. They keep their allocations from
    /// batch to batch, because those are large and take time to grow: see the [module
    /// documentation](self#batches-and-threads).
    workers: Vec<Worker<S>>,
    /// How many routes the current iteration's batches have committed.
    commits: u64,
    /// For resources that a commit left with no room for another net, the value of `commits`
    /// after the latest such commit. A batch route that uses one, and began before that, has
    /// a conflict. It is only ever looked up.
    taken: FastMap<S::Resource, u64>,
    /// For each batch, the most expansions of any member and of all members together, which
    /// show how much any number of threads could have gained.
    #[cfg(test)]
    batch_work: Vec<(u64, u64)>,
    /// Indices into `nets`, in the order they take their turns.
    order: Vec<u32>,
    rng: Option<Rng>,
    /// The present factor of the most recent iteration.
    present: Cost,
    iteration: u32,
    stats: RouteStats,
    reports: Vec<IterationReport>,
    best: Option<Best<S::Node, S::Resource>>,
    stop: Option<StopReason>,
    timing: Option<TimingState<S>>,
    /// The first iteration that left nothing overused but what fixed routes cause.
    first_legal: Option<u32>,
    /// Whether the current iteration has changed any route, which is all that it needs to know.
    changed: bool,
}

impl<S: RoutingSpace> fmt::Debug for Engine<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("nets", &self.nets.len())
            .field("iteration", &self.iteration)
            .field("stop", &self.stop)
            .finish_non_exhaustive()
    }
}

impl<'a, S: RoutingSpace> Engine<'a, S> {
    /// Creates an engine, which does no work until it is stepped.
    ///
    /// # Errors
    ///
    /// Returns an error if two nets have the same ID.
    pub fn new(
        space: &'a S,
        nets: &'a [Net<S::Node>],
        options: EngineOptions<'a>,
    ) -> Result<Self, EngineError> {
        let mut ids: Vec<NetId> = nets.iter().map(|net| net.id).collect();
        ids.sort_unstable();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(EngineError::DuplicateNet { net: pair[0] });
        }

        let states = nets
            .iter()
            .map(|net| NetState {
                route: None,
                held: Vec::new(),
                resources: Vec::new(),
                cost: Cost::ZERO,
                delays: Vec::new(),
                dead_roots: Vec::new(),
                no_path: has_terminal_outside(space, net),
                fixed: None,
            })
            .collect();
        Ok(Engine {
            space,
            nets,
            options,
            table: ResourceTable::new(space),
            states,
            workers: vec![Worker::new(space)],
            commits: 0,
            taken: FastMap::default(),
            #[cfg(test)]
            batch_work: Vec::new(),
            order: (0..nets.len() as u32).collect(),
            rng: options.shuffle_seed.map(Rng::new),
            present: Cost::ZERO,
            iteration: 0,
            stats: RouteStats::default(),
            reports: Vec::new(),
            best: None,
            stop: None,
            timing: None,
            first_legal: None,
            changed: false,
        })
    }

    /// Fixes routes that the caller has decided on, before the first iteration. See [fixed and
    /// pre-routed nets](self#fixed-and-pre-routed-nets).
    ///
    /// A route that reaches every terminal of its net fixes the net, which is never routed.
    /// One that does not is grown to the rest by the engine, which only ever changes what it
    /// added. Either way the route holds its resources from now on, at the price of present
    /// usage for every other net, and appears in the results. In a timing-driven run, the
    /// delays along it replace the estimates of its connections, and criticalities are worked
    /// out again.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::InvalidFixedRoute`] for the first route that cannot be taken, in
    /// the order given, and then takes none of them. A route has to be a tree directed away from
    /// its root, rooted at an access point of the driver if the net needs routing, and made of
    /// steps of the space. A net needing no routing has to have its terminal on the route. Each
    /// net can have one fixed route, and they have to be given before the first iteration.
    ///
    /// # Examples
    ///
    /// A clock net fixed along the middle row of a grid, and a net that has to cross it. The
    /// crossing net goes around through the second layer, and the clock is untouched.
    ///
    /// ```
    /// use guide::engine::{Engine, EngineOptions};
    /// use guide::id::NetId;
    /// use guide::net::{FixedRoute, Net, Route, Terminal};
    /// use guide::space::grid::Grid;
    ///
    /// let grid = Grid::new(5, 5, 2)?;
    /// let node = |x, y| grid.node_at(x, y, 0).unwrap();
    /// let nets = [
    ///     Net { id: NetId(0), terminals: vec![Terminal::at(node(0, 2)), Terminal::at(node(4, 2))] },
    ///     Net { id: NetId(1), terminals: vec![Terminal::at(node(2, 0)), Terminal::at(node(2, 4))] },
    /// ];
    /// let mut clock = Route::new();
    /// clock.add_path(&(0..5).map(|x| node(x, 2)).collect::<Vec<_>>()).unwrap();
    ///
    /// let mut engine = Engine::new(&grid, &nets, EngineOptions::default()).unwrap();
    /// engine.fix_routes([FixedRoute { net: NetId(0), route: clock.clone() }]).unwrap();
    /// let outcome = engine.run();
    ///
    /// assert!(outcome.is_success());
    /// assert_eq!(outcome.routes.route(NetId(0)), Some(&clock));
    /// assert!(!outcome.routes.route(NetId(1)).unwrap().contains(node(2, 2)));
    /// # Ok::<(), guide::space::grid::GridError>(())
    /// ```
    pub fn fix_routes(
        &mut self,
        routes: impl IntoIterator<Item = FixedRoute<S::Node>>,
    ) -> Result<(), EngineError> {
        // Looked up and never iterated.
        let index_of: FastMap<NetId, usize> = self
            .nets
            .iter()
            .enumerate()
            .map(|(index, net)| (net.id, index))
            .collect();
        let mut given = vec![false; self.nets.len()];
        let mut parts = Vec::new();
        for FixedRoute { net, route } in routes {
            let invalid = |problem| EngineError::InvalidFixedRoute { net, problem };
            if self.iteration > 0 {
                return Err(invalid(FixedRouteProblem::AlreadyStarted));
            }
            let index = *index_of
                .get(&net)
                .ok_or(invalid(FixedRouteProblem::UnknownNet))?;
            if given[index] || self.states[index].fixed.is_some() {
                return Err(invalid(FixedRouteProblem::Repeated));
            }
            given[index] = true;
            let part = self
                .prepare_fixed(&self.nets[index], route)
                .map_err(invalid)?;
            parts.push((index, part));
        }
        if parts.is_empty() {
            return Ok(());
        }

        for (index, mut part) in parts {
            let state = &mut self.states[index];
            // A terminal outside the space can't be reached by a route, fixed or not, so the
            // net has no path and its fixed part goes with it, as for any that can't be
            // completed.
            if state.no_path {
                continue;
            }
            let slots: Vec<usize> = part
                .resources
                .iter()
                .map(|&resource| self.table.intern(self.space, resource))
                .collect();
            self.table.acquire(&slots);
            self.table.mark_fixed(&slots, true);
            part.slots.clone_from(&slots);
            part.slots.sort_unstable();
            state.held = slots;
            state.resources.clone_from(&part.resources);
            if part.complete {
                state.route = Some(part.route.clone());
                state.cost = part.cost;
                state.delays.clone_from(&part.delays);
            }
            state.fixed = Some(Box::new(part));
        }
        if let Some(timing) = &mut self.timing {
            timing.analyse(&self.states);
            timing.refresh_criticality();
        }
        Ok(())
    }

    /// Checks a fixed route of `net`, and works out what the engine needs to know about it.
    fn prepare_fixed(
        &self,
        net: &Net<S::Node>,
        route: Route<S::Node>,
    ) -> Result<FixedPart<S::Node, S::Resource>, FixedRouteProblem> {
        let space = self.space;
        let Some(root) = route.root() else {
            return Err(FixedRouteProblem::Empty);
        };
        route
            .validate_tree()
            .map_err(|_| FixedRouteProblem::NotATree)?;
        let driver = net.driver().map_or(&[][..], Terminal::access_points);
        let root_access = driver
            .iter()
            .filter(|point| point.node == root)
            .map(|point| point.cost)
            .min();
        if net.needs_routing() && root_access.is_none() {
            return Err(FixedRouteProblem::RootNotOnDriver);
        }
        if route.nodes().any(|node| !space.contains(node)) {
            return Err(FixedRouteProblem::NodeNotInSpace);
        }

        let step_delays = self.timing.as_ref().map(|timing| timing.step_delays);
        let mut cost = root_access.unwrap_or(Cost::ZERO);
        let mut resources = Vec::new();
        space.resources(net.id, root, root, &mut resources);
        let (mut steps, mut delays) = (Vec::new(), Vec::new());
        let mut edge_delays = Vec::new();
        for (index, edge) in route.edges().iter().enumerate() {
            steps.clear();
            space.expand(net.id, edge.from, &mut steps);
            let Some(position) = steps.iter().position(|step| step.to == edge.to) else {
                return Err(FixedRouteProblem::NotAStep { index });
            };
            cost = cost + steps[position].cost;
            space.resources(net.id, edge.from, edge.to, &mut resources);
            if let Some(step_delays) = step_delays {
                delays.clear();
                step_delays(space, net.id, edge.from, &mut delays);
                edge_delays.push(delays.get(position).copied().unwrap_or(0));
            }
        }
        resources.sort_unstable();
        resources.dedup();

        // In a tree, each node but the root has one parent edge, so a walk down from the root
        // reaches every node once, after its parent, whatever the order of the edges.
        let mut arrivals = Vec::new();
        if step_delays.is_some() {
            let mut children: FastMap<S::Node, Vec<usize>> = FastMap::default();
            for (index, edge) in route.edges().iter().enumerate() {
                children.entry(edge.from).or_default().push(index);
            }
            let mut stack = vec![(root, 0u32)];
            while let Some((node, arrival)) = stack.pop() {
                arrivals.push((node, arrival));
                for &index in children.get(&node).map_or(&[][..], Vec::as_slice) {
                    let delay = arrival.saturating_add(edge_delays[index]);
                    stack.push((route.edges()[index].to, delay));
                }
            }
        }
        // Looked up and never iterated.
        let arrival_at: FastMap<S::Node, u32> = arrivals.iter().copied().collect();
        let on_route: FastSet<S::Node> = route.nodes().collect();
        if space.has_non_configurable() {
            let mut tied = Vec::new();
            for node in route.nodes() {
                tied.clear();
                space.non_configurable(net.id, node, &mut tied);
                if tied.iter().any(|to| !on_route.contains(to)) {
                    return Err(FixedRouteProblem::NonConfigurableMissing);
                }
            }
        }

        if !net.needs_routing()
            && let Some(only) = net.terminals.first()
            && !only
                .access_points()
                .iter()
                .any(|point| on_route.contains(&point.node))
        {
            return Err(FixedRouteProblem::MissingTerminal);
        }
        let mut pending = Vec::new();
        let mut terminal_delays = vec![
            0;
            if step_delays.is_some() {
                net.terminals.len()
            } else {
                0
            }
        ];
        for (index, terminal) in net.terminals.iter().enumerate().skip(1) {
            let reached = || {
                terminal
                    .access_points()
                    .iter()
                    .filter(|point| on_route.contains(&point.node))
            };
            match reached().map(|point| point.cost).min() {
                Some(access) => {
                    cost = cost + access;
                    if step_delays.is_some() {
                        let delay = reached()
                            .filter_map(|point| arrival_at.get(&point.node))
                            .min();
                        terminal_delays[index] = delay.copied().unwrap_or(0);
                    }
                }
                None => pending.push(index),
            }
        }

        Ok(FixedPart {
            route,
            resources,
            slots: Vec::new(),
            cost,
            complete: pending.is_empty(),
            pending,
            arrivals,
            delays: terminal_delays,
        })
    }

    /// Runs one iteration and reports on it, or returns `None` if the engine has stopped.
    ///
    /// The iteration that causes the stop is still reported, so the `None` comes from the
    /// call after that one. The exceptions are a cancel flag that is already set between
    /// iterations and an iteration limit of zero, which stop without an iteration to report.
    ///
    /// This uses one thread, and gives the same result as
    /// [`step_parallel`](Engine::step_parallel) does with any number of them.
    pub fn step(&mut self) -> Option<IterationReport> {
        if !self.begin_iteration() {
            return None;
        }
        let before = self.stats;
        let turns = if self.options.batch_size > 1 {
            self.take_turns_in_batches(|engine, jobs| engine.route_batch_here(jobs))
        } else {
            self.take_turns()
        };
        Some(self.finish_iteration(before, turns))
    }

    /// Prepares the next iteration, or returns false if there is not to be one.
    fn begin_iteration(&mut self) -> bool {
        if self.stop.is_some() {
            return false;
        }
        if self.cancelled() {
            self.stop = Some(StopReason::Cancelled);
            return false;
        }
        if self.iteration >= self.options.max_iterations {
            self.stop = Some(StopReason::IterationLimit);
            return false;
        }

        self.iteration += 1;
        self.present = self.next_present();
        self.changed = false;
        if self.iteration > 1
            && let Some(rng) = &mut self.rng
        {
            rng.shuffle(&mut self.order);
        }
        true
    }

    /// Gives every net its turn, each seeing the commit of the one before.
    fn take_turns(&mut self) -> Turns {
        let mut nets_rerouted = 0;
        let mut interrupted = None;
        for position in 0..self.order.len() {
            let index = self.order[position] as usize;
            if !self.wants_routing(index) {
                continue;
            }
            if self.cancelled() {
                interrupted = Some(Interrupt::Cancel);
                break;
            }
            nets_rerouted += 1;
            if let Err(interrupt) = self.reroute(index) {
                interrupted = Some(interrupt);
                break;
            }
        }
        Turns {
            nets_rerouted,
            interrupted,
            batches: nets_rerouted,
            discarded: 0,
        }
    }

    /// Gives every net its turn with routes from batches, which `route_batch` provides: it
    /// routes the given nets against the current table, by whatever means it likes.
    ///
    /// Everything that decides the result happens here, on one thread, in the order of the
    /// nets. That is what leaves the thread count out of it, as long as which nets go in a
    /// batch only depends on what is decided here.
    fn take_turns_in_batches(
        &mut self,
        mut route_batch: impl FnMut(&mut Self, &[u32]) -> RoutedBatch<S>,
    ) -> Turns {
        let batch_size = self.options.batch_size.max(1) as usize;
        let mut turns = Turns::default();
        // Looked up by net and never iterated.
        let mut waiting = FastMap::default();
        let mut jobs = Vec::with_capacity(batch_size);
        let mut position = 0;
        self.commits = 0;
        self.taken.clear();

        while position < self.order.len() {
            // A batch is the next nets that want routing now. Those with a route still waiting
            // from an earlier batch count towards its size.
            jobs.clear();
            let mut filled = 0;
            for &index in &self.order[position..] {
                if filled == batch_size {
                    break;
                }
                if waiting.contains_key(&index) {
                    filled += 1;
                } else if self.wants_routing(index as usize) {
                    jobs.push(index);
                    filled += 1;
                }
            }
            if filled == 0 {
                break;
            }
            if !jobs.is_empty() {
                turns.batches += 1;
                waiting.extend(route_batch(self, &jobs));
                #[cfg(test)]
                {
                    let work = jobs.iter().map(|job| waiting[job].expanded);
                    let (most, total) = work.fold((0, 0), |(m, t), e: u64| (m.max(e), t + e));
                    self.batch_work.push((most, total));
                }
            }

            while position < self.order.len() {
                let index = self.order[position];
                if !self.wants_routing(index as usize) {
                    // An earlier net settled this one's conflict while it was being routed.
                    if let Some(unwanted) = waiting.remove(&index) {
                        turns.discarded += 1;
                        if let Err(interrupt) = self.charge(&unwanted) {
                            turns.interrupted = Some(interrupt);
                            return turns;
                        }
                    }
                    position += 1;
                    continue;
                }
                if self.cancelled() {
                    turns.interrupted = Some(Interrupt::Cancel);
                    return turns;
                }
                let Some(routed) = waiting.remove(&index) else {
                    // It only wants routing because of a commit since the batch was chosen.
                    break;
                };
                // Stopped by a flag that has since been cleared, which says nothing about it.
                if matches!(routed.attempt, Attempt::Interrupted(Interrupt::Cancel)) {
                    break;
                }
                if self.has_conflict(&routed) {
                    turns.discarded += 1;
                    if let Err(interrupt) = self.charge(&routed) {
                        turns.interrupted = Some(interrupt);
                        return turns;
                    }
                    break;
                }

                turns.nets_rerouted += 1;
                position += 1;
                if let Err(interrupt) = self.charge(&routed) {
                    turns.interrupted = Some(interrupt);
                    return turns;
                }
                self.commit(index as usize, routed);
            }
            if waiting.is_empty() {
                // Nothing is left that began before these, and the map only costs lookups.
                self.taken.clear();
            }
        }
        turns
    }

    /// Returns whether an earlier net of the iteration has taken something that `routed` uses
    /// since the route was found. A resource given up in that time can only have made the
    /// route better than it looked, so that is no conflict.
    fn has_conflict(&self, routed: &BatchRoute<S::Node, S::Resource>) -> bool {
        let Attempt::Routed { resources, .. } = &routed.attempt else {
            return false;
        };
        resources.iter().any(|resource| {
            self.taken
                .get(resource)
                .is_some_and(|&commit| commit > routed.commits_seen)
        })
    }

    /// Takes the work of a batch route out of the budget, in the order of the nets. This is
    /// the only place that happens, so that the workers cannot race for what is left.
    ///
    /// A route that would overrun the budget stops the run as if it had been stopped when
    /// the budget ran out, whichever thread found it and however far it got.
    fn charge(&mut self, routed: &BatchRoute<S::Node, S::Resource>) -> Result<(), Interrupt> {
        let budget = self.options.max_expansions;
        self.stats.searches += routed.searches;
        let total = self.stats.nodes_expanded.saturating_add(routed.expanded);
        self.stats.nodes_expanded = total.min(budget);
        if total > budget || matches!(routed.attempt, Attempt::Interrupted(Interrupt::Budget)) {
            return Err(Interrupt::Budget);
        }
        Ok(())
    }

    /// Puts a batch route into the table.
    fn commit(&mut self, index: usize, routed: BatchRoute<S::Node, S::Resource>) {
        let state = &mut self.states[index];
        self.table.release(&state.held);
        state.held.clear();
        state.dead_roots = routed.dead_roots;
        match routed.attempt {
            Attempt::Routed {
                route,
                cost,
                resources,
                delays,
            } => {
                self.commits += 1;
                for &resource in &resources {
                    let slot = self.table.intern(self.space, resource);
                    state.held.push(slot);
                    let table_state = self.table.states[slot];
                    // A resource on the old route as well has gained nothing. One with room
                    // left over after this net can take another without any overuse.
                    if state.resources.binary_search(&resource).is_err()
                        && table_state.occupancy.saturating_add(1) >= table_state.capacity
                    {
                        self.taken.insert(resource, self.commits);
                    }
                }
                self.table.acquire(&state.held);
                state.resources = resources;
                self.changed |= state.route.as_ref() != Some(&route);
                state.route = Some(route);
                state.cost = cost;
                state.delays = delays;
            }
            Attempt::NoPath => {
                state.resources.clear();
                state.route = None;
                state.cost = Cost::ZERO;
                state.delays.clear();
                state.no_path = true;
                // A fixed part that cannot be completed goes with the net: see the module
                // documentation.
                if let Some(fixed) = state.fixed.take() {
                    self.table.mark_fixed(&fixed.slots, false);
                }
            }
            // Budgets stop before this, and a cancelled route is not committed.
            Attempt::Interrupted(_) => unreachable!("an interrupted route is not committed"),
        }
    }

    /// Returns what the members of a batch share.
    fn batch_context(&self) -> BatchContext<'_, S> {
        BatchContext {
            route: RouteContext {
                space: self.space,
                table: &self.table,
                present: self.present,
                cancel: self.options.cancel,
                timing: self.timing.as_ref().map(TimingState::route_timing),
            },
            nets: self.nets,
            states: &self.states,
            budget: self
                .options
                .max_expansions
                .saturating_sub(self.stats.nodes_expanded),
            commits: self.commits,
        }
    }

    /// Routes a batch on the calling thread, for a space that threads cannot share.
    fn route_batch_here(&mut self, jobs: &[u32]) -> RoutedBatch<S> {
        // Taken out for now so that the rest of the engine can be borrowed as a whole.
        let mut workers = core::mem::take(&mut self.workers);
        let context = self.batch_context();
        let routed = jobs
            .iter()
            .map(|&index| (index, route_in_batch(&context, index, &mut workers[0])))
            .collect();
        self.workers = workers;
        routed
    }

    /// Does the accounting at the end of an iteration.
    fn finish_iteration(&mut self, before: RouteStats, turns: Turns) -> IterationReport {
        let Turns {
            nets_rerouted,
            interrupted,
            batches,
            discarded,
        } = turns;
        self.stats.iterations += 1;

        let (overused_resources, total_overuse, fixed_overuse) = self.table.overuse();
        let unrouted = self.unrouted_count();
        // Summed in net order, not turn order, so that shuffling cannot change the rounding
        // of the total for the same set of routes.
        let total_cost: Cost = self
            .states
            .iter()
            .filter(|state| state.route.is_some())
            .map(|state| state.cost)
            .sum();
        let timing = self.timing.as_mut().map(|timing| {
            timing.analyse(&self.states);
            timing.metrics
        });
        self.consider_best(interrupted, unrouted, total_overuse, total_cost);
        // Only now, so that the best iteration's criticalities were the ones it routed with,
        // and so that the next iteration has a single set for all of its nets.
        if let Some(timing) = &mut self.timing {
            timing.refresh_criticality();
        }

        let report = IterationReport {
            iteration: self.iteration,
            present: self.present,
            completed: interrupted.is_none(),
            nets_rerouted,
            searches: self.stats.searches - before.searches,
            nodes_expanded: self.stats.nodes_expanded - before.nodes_expanded,
            unrouted,
            overused_resources,
            total_overuse,
            fixed_overuse,
            total_cost,
            batches,
            discarded,
            timing,
        };
        self.reports.push(report);

        // What is left, if anything, is between fixed routes, which no iteration can change.
        let legal = total_overuse == fixed_overuse;
        let improving = self.improves_timing();
        if legal && interrupted.is_none() && self.first_legal.is_none() {
            self.first_legal = Some(self.iteration);
        }
        // Once nothing changes, nothing will: the next iteration would route the same nets
        // against the same table, and find the same routes.
        let improved_enough = self.first_legal.is_some_and(|first| {
            self.iteration - first >= self.options.timing_iterations
                || (legal
                    && (!self.changed
                        || !self.timing.as_ref().is_some_and(TimingState::any_critical)))
        });
        if let Some(interrupt) = interrupted {
            self.stop = Some(interrupt.stop());
        } else if (legal && !improving) || (improving && improved_enough) {
            self.stop = Some(StopReason::Converged);
        } else if self.iteration >= self.options.max_iterations {
            // A run that found a legal result has one, whatever this iteration did.
            let found = improving && self.first_legal.is_some();
            self.stop = Some(if found {
                StopReason::Converged
            } else {
                StopReason::IterationLimit
            });
        } else if !legal {
            self.table.add_history(self.options.history_factor);
        }
        report
    }

    /// Whether the run goes on after its first legal result, to improve its timing.
    fn improves_timing(&self) -> bool {
        self.timing.is_some() && self.options.timing_iterations > 0
    }

    /// Steps until the engine stops, and returns the result.
    pub fn run(mut self) -> Outcome<S::Node, S::Resource> {
        while self.step().is_some() {}
        self.outcome()
    }

    /// Returns why the engine stopped, or `None` if it can still make progress.
    pub fn stop_reason(&self) -> Option<StopReason> {
        self.stop
    }

    /// Returns the reports of all iterations so far.
    pub fn reports(&self) -> &[IterationReport] {
        &self.reports
    }

    /// Returns the criticality that the next iteration gives a connection, by its index in
    /// [`TimingDriven::connections`], or `None` if there is no such connection or the run is
    /// not timing-driven.
    pub fn connection_criticality(&self, connection: usize) -> Option<f64> {
        let timing = self.timing.as_ref()?;
        let &(_, _, slot) = timing.places.get(connection)?;
        Some(timing.criticality[slot])
    }

    /// Returns the timing graph, with the delays of the latest routes, or `None` if the run is
    /// not timing-driven.
    pub fn timing_graph(&self) -> Option<&TimingGraph> {
        self.timing.as_ref().map(|timing| &timing.graph)
    }

    /// Returns the analysis of the latest routes, or `None` if the run is not timing-driven.
    pub fn timing_analysis(&self) -> Option<&TimingAnalysis> {
        self.timing.as_ref().map(|timing| &timing.analysis)
    }

    /// Returns the resources overused right now, which need not be by the best iteration.
    pub fn overused(&self) -> Vec<OverusedResource<S::Resource>> {
        self.collect_overused()
    }

    /// Returns the best result so far, which the engine keeps.
    pub fn outcome(&self) -> Outcome<S::Node, S::Resource> {
        let stop = self.stop.unwrap_or(StopReason::NotFinished);
        let (mut routes, overused, best_iteration, total_cost, timing) = match &self.best {
            Some(best) => (
                best.routes.clone(),
                best.overused.clone(),
                best.iteration,
                best.total_cost,
                best.timing.clone(),
            ),
            None => {
                let reason = match stop {
                    StopReason::IterationLimit | StopReason::BudgetExhausted => {
                        UnroutedReason::BudgetExhausted
                    }
                    _ => UnroutedReason::Cancelled,
                };
                let timing = self.timing.as_ref().map(|t| t.outcome(&self.states));
                (
                    self.current_routes(reason),
                    Vec::new(),
                    0,
                    Cost::ZERO,
                    timing,
                )
            }
        };
        routes.stats = self.stats;
        Outcome {
            routes,
            overused,
            stop,
            best_iteration,
            total_cost,
            timing,
        }
    }

    fn cancelled(&self) -> bool {
        self.options
            .cancel
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Returns the present factor for the iteration about to start.
    fn next_present(&self) -> Cost {
        let options = &self.options;
        let restart = self.first_legal.is_some()
            && self
                .reports
                .last()
                .is_some_and(|report| report.total_overuse == report.fixed_overuse);
        let present = match self.iteration {
            0 | 1 => options.first_present,
            2 => options.initial_present,
            _ if restart => options.initial_present,
            _ => self.present * options.present_growth,
        };
        present.min(options.max_present)
    }

    /// Returns whether a net should be routed again now.
    fn wants_routing(&self, index: usize) -> bool {
        let state = &self.states[index];
        if !self.nets[index].needs_routing() || state.no_path || state.is_fixed() {
            return false;
        }
        if state.route.is_none() || self.options.reroute_all {
            return true;
        }
        if self.first_legal.is_some()
            && self.improves_timing()
            && self
                .timing
                .as_ref()
                .is_some_and(|timing| timing.critical_nets[index])
        {
            return true;
        }
        match &state.fixed {
            None => state
                .held
                .iter()
                .any(|&slot| self.table.avoidable_overuse(slot) > 0),
            // Routing again can't move what is fixed.
            Some(fixed) => state.held.iter().any(|&slot| {
                self.table.avoidable_overuse(slot) > 0 && fixed.slots.binary_search(&slot).is_err()
            }),
        }
    }

    /// Rips up a net and routes it again, or puts it back if interrupted.
    fn reroute(&mut self, index: usize) -> Result<(), Interrupt> {
        let net = &self.nets[index];
        let state = &mut self.states[index];
        self.table.release(&state.held);

        let context = RouteContext {
            space: self.space,
            table: &self.table,
            present: self.present,
            cancel: self.options.cancel,
            timing: self.timing.as_ref().map(TimingState::route_timing),
        };
        let criticality = context.timing.map_or(&[][..], |t| t.of_net(index));
        let budget = self.options.max_expansions;
        let attempt = route_net(
            &context,
            net,
            criticality,
            budget.saturating_sub(self.stats.nodes_expanded),
            &mut state.dead_roots,
            state.fixed.as_deref(),
            &mut self.workers[0],
        );
        let worker = &mut self.workers[0];
        self.stats.searches += worker.searches;
        self.stats.nodes_expanded += worker.expanded;
        worker.searches = 0;
        worker.expanded = 0;

        match attempt {
            Attempt::Routed {
                route,
                cost,
                resources,
                delays,
            } => {
                state.held.clear();
                for &resource in &resources {
                    state.held.push(self.table.intern(self.space, resource));
                }
                self.table.acquire(&state.held);
                state.resources = resources;
                self.changed |= state.route.as_ref() != Some(&route);
                state.route = Some(route);
                state.cost = cost;
                state.delays = delays;
                Ok(())
            }
            Attempt::NoPath => {
                state.held.clear();
                state.resources.clear();
                state.route = None;
                state.cost = Cost::ZERO;
                state.delays.clear();
                state.no_path = true;
                if let Some(fixed) = state.fixed.take() {
                    self.table.mark_fixed(&fixed.slots, false);
                }
                Ok(())
            }
            Attempt::Interrupted(interrupt) => {
                self.table.acquire(&state.held);
                Err(interrupt)
            }
        }
    }

    fn unrouted_count(&self) -> u32 {
        let unrouted =
            self.nets.iter().zip(&self.states).filter(|(net, state)| {
                (net.needs_routing() || state.no_path) && state.route.is_none()
            });
        unrouted.count() as u32
    }

    /// Keeps the current state if it is the best so far.
    fn consider_best(
        &mut self,
        interrupted: Option<Interrupt>,
        unrouted: u32,
        total_overuse: u64,
        total_cost: Cost,
    ) {
        let metrics = self.timing.as_ref().map(|timing| &timing.metrics);
        let rank = rank(
            unrouted,
            total_overuse,
            metrics,
            self.improves_timing(),
            total_cost,
        );
        let better = self.best.as_ref().is_none_or(|best| rank < best.rank);
        if better {
            let reason = interrupted.map_or(UnroutedReason::BudgetExhausted, Interrupt::reason);
            self.best = Some(Best {
                routes: self.current_routes(reason),
                overused: self.collect_overused(),
                iteration: self.iteration,
                rank,
                total_cost,
                timing: self.timing.as_ref().map(|t| t.outcome(&self.states)),
            });
        }
    }

    /// Returns the current routes, giving `reason` for nets that have no route for no worse
    /// reason than not having been reached.
    fn current_routes(&self, reason: UnroutedReason) -> RouteSet<S::Node> {
        let mut routes = RouteSet::new();
        for (net, state) in self.nets.iter().zip(&self.states) {
            if state.no_path {
                routes.set_unrouted(net.id, UnroutedReason::NoPath);
            } else if !net.needs_routing() && state.route.is_none() {
                routes.set_route(net.id, Route::new());
            } else if let Some(route) = &state.route {
                routes.set_route(net.id, route.clone());
            } else {
                routes.set_unrouted(net.id, reason);
            }
        }
        routes
    }

    fn collect_overused(&self) -> Vec<OverusedResource<S::Resource>> {
        let table = &self.table;
        // Where each slot's entry is in `found`, so that nets can be matched to entries in a
        // single pass. Overused resources are few, and this is only looked up.
        let mut positions = FastMap::default();
        let mut found = Vec::new();
        for (&slot, &resource) in table.occupied_slots.iter().zip(&table.occupied_resources) {
            let state = table.states[slot];
            if state.overuse() > 0 {
                positions.insert(slot, found.len());
                found.push(OverusedResource {
                    resource,
                    occupancy: state.occupancy,
                    capacity: state.capacity,
                    nets: Vec::new(),
                    fixed: table.fixed_at(slot),
                });
            }
        }
        if found.is_empty() {
            return found;
        }

        for (net, state) in self.nets.iter().zip(&self.states) {
            for slot in &state.held {
                if let Some(&position) = positions.get(slot) {
                    let entry: &mut OverusedResource<S::Resource> = &mut found[position];
                    entry.nets.push(net.id);
                }
            }
        }
        for entry in &mut found {
            entry.nets.sort_unstable();
        }
        found.sort_unstable_by_key(|entry| entry.resource);
        found
    }
}

impl<'a, S: DelaySpace> Engine<'a, S> {
    /// Creates a timing-driven engine, which does no work until it is stepped. See
    /// [timing-driven routing](self#timing-driven-routing).
    ///
    /// The timing graph is analysed once here, with the delays it was built with, for the
    /// criticalities of the first iteration.
    ///
    /// # Errors
    ///
    /// Returns an error if two nets have the same ID, or if a connection names a net that is
    /// not among `nets`, a terminal that is not a sink of it, an arc that is not a net arc, or a
    /// sink or an arc that another connection already named.
    ///
    /// # Examples
    ///
    /// Two ways from 0 to 3: a cheap and slow one through 1, and a dear and fast one through 2.
    /// The only connection is on the only path, so it is fully critical, up to VPR's maximum
    /// of 0.99, and it takes the fast way.
    ///
    /// ```
    /// use guide::cost::Cost;
    /// use guide::engine::{Connection, Engine, EngineOptions, TimingDriven};
    /// use guide::id::{NetId, NodeId};
    /// use guide::net::{Net, Terminal};
    /// use guide::space::graph::GraphBuilder;
    /// use guide::timing::{Constraints, TimingGraphBuilder};
    ///
    /// let mut builder = GraphBuilder::new(4);
    /// for (from, to, cost) in [(0, 1, 1), (1, 3, 1), (0, 2, 3), (2, 3, 3)] {
    ///     builder.add_edge(NodeId(from), NodeId(to), Cost::from(cost))?;
    /// }
    /// let graph = builder.build();
    /// // 500 ps a step through node 1, and 100 through node 2.
    /// let space = graph.with_step_delays(|from, to| if from.0 == 1 || to.0 == 1 { 500 } else { 100 });
    ///
    /// let mut timing = TimingGraphBuilder::new();
    /// let (driver, sink) = (timing.add_input(0)?, timing.add_output(0)?);
    /// let arc = timing.add_net_arc(driver, sink, 0)?;
    /// let connections = vec![Connection { net: NetId(0), terminal: 1, arc }];
    /// let setup = TimingDriven::new(
    ///     timing.build(),
    ///     Constraints::single_clock(0)?,
    ///     connections,
    ///     Cost::new(0.01).unwrap(),
    /// );
    ///
    /// let nets = [Net { id: NetId(0), terminals: vec![Terminal::at(NodeId(0)), Terminal::at(NodeId(3))] }];
    /// let mut engine = Engine::with_timing(&space, &nets, EngineOptions::default(), setup).unwrap();
    /// assert_eq!(engine.connection_criticality(0), Some(0.99));
    ///
    /// let report = engine.step().unwrap();
    /// assert_eq!(report.timing.unwrap().critical_path_delay, Some(200));
    /// let outcome = engine.run();
    /// assert!(outcome.is_success());
    /// assert!(outcome.routes.route(NetId(0)).unwrap().contains(NodeId(2)));
    /// assert_eq!(outcome.timing.unwrap().connection_delays, [Some(200)]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_timing(
        space: &'a S,
        nets: &'a [Net<S::Node>],
        options: EngineOptions<'a>,
        timing: TimingDriven,
    ) -> Result<Self, EngineError> {
        let mut engine = Engine::new(space, nets, options)?;
        engine.timing = Some(TimingState::new(
            nets,
            timing,
            S::step_delays,
            S::delay_lower_bound,
            options.timing_reroute_criticality,
        )?);
        Ok(engine)
    }
}

impl<'a, S> Engine<'a, S>
where
    S: RoutingSpace + Sync,
    S::Node: Send + Sync,
    S::Resource: Send + Sync,
    S::Slots: Send,
    S::ResourceSlots: Sync,
{
    /// Runs one iteration like [`step`](Engine::step), with up to `threads` threads searching
    /// at once. A value of 0 means 1.
    ///
    /// The thread count can differ from one call to the next, and no choice of them changes
    /// the result. The workers a call creates stay for later calls, whatever those ask for, at
    /// [44 bytes per node each](self#memory).
    ///
    /// # Panics
    ///
    /// A panic in a space's methods on any thread reaches the caller, and leaves the engine
    /// unfit for further use.
    pub fn step_parallel(&mut self, threads: usize) -> Option<IterationReport> {
        if !self.begin_iteration() {
            return None;
        }
        let before = self.stats;
        let turns = if self.options.batch_size > 1 {
            self.take_turns_in_batches(|engine, jobs| engine.route_batch_on_threads(jobs, threads))
        } else {
            self.take_turns()
        };
        Some(self.finish_iteration(before, turns))
    }

    /// Steps with up to `threads` threads until the engine stops, and returns the result.
    pub fn run_parallel(mut self, threads: usize) -> Outcome<S::Node, S::Resource> {
        while self.step_parallel(threads).is_some() {}
        self.outcome()
    }

    /// Routes a batch with up to `threads` threads, in no particular order.
    ///
    /// Threads are started for each batch because the table cannot be borrowed by a thread
    /// that outlives it, and commits have to change it between batches. Sharing it for longer
    /// would take a lock on the hottest read in the crate, or `unsafe`. A batch takes
    /// milliseconds in the cases that are worth parallelising, against tens of microseconds to
    /// start a thread.
    fn route_batch_on_threads(&mut self, jobs: &[u32], threads: usize) -> RoutedBatch<S> {
        let threads = threads.clamp(1, jobs.len().max(1));
        if threads == 1 {
            return self.route_batch_here(jobs);
        }
        while self.workers.len() < threads {
            self.workers.push(Worker::new(self.space));
        }

        // Taken out for now so that the rest of the engine can be borrowed as a whole.
        let mut workers = core::mem::take(&mut self.workers);
        let context = self.batch_context();
        let next_job = AtomicUsize::new(0);
        // Taking jobs as they come balances searches of very different sizes. Which worker gets
        // which does not matter, as a search leaves nothing behind in its worker that shows.
        let work_through = |worker: &mut Worker<S>| {
            let mut routed = Vec::new();
            while let Some(&index) = jobs.get(next_job.fetch_add(1, Ordering::Relaxed)) {
                routed.push((index, route_in_batch(&context, index, worker)));
            }
            routed
        };
        let (first, helpers) = workers.split_at_mut(1);
        let routed = std::thread::scope(|scope| {
            let running: Vec<_> = helpers[..threads - 1]
                .iter_mut()
                .map(|helper| scope.spawn(|| work_through(helper)))
                .collect();
            let mut routed = work_through(&mut first[0]);
            for thread in running {
                match thread.join() {
                    Ok(more) => routed.extend(more),
                    // As it is and not the scope's own message, so that the caller gets the
                    // space's panic. The other threads run out of jobs and finish first.
                    Err(panic) => resume_unwind(panic),
                }
            }
            routed
        });
        self.workers = workers;
        routed
    }
}

/// Returns whether any terminal of `net` has no access point in the space, so that no route
/// can connect it.
fn has_terminal_outside<S: RoutingSpace>(space: &S, net: &Net<S::Node>) -> bool {
    net.terminals.iter().any(|terminal| {
        !terminal
            .access_points()
            .iter()
            .any(|point| space.contains(point.node))
    })
}

#[cfg(test)]
mod parallel_tests;

#[cfg(test)]
mod timing_tests;

#[cfg(test)]
mod fixed_tests;

#[cfg(test)]
mod tied_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::NodeId;
    use crate::net::{AccessPoint, NetOutcome, Terminal};
    use crate::space::graph::{Graph, GraphBuilder};
    use crate::space::grid::Grid;
    use crate::space::{DenseSlots, InternedSlots};
    use core::cell::Cell;
    use core::fmt::Debug;
    use proptest::prelude::*;
    use std::collections::{BTreeMap, BTreeSet, HashSet};

    // Helpers.

    fn net(id: u32, terminals: &[u32]) -> Net<NodeId> {
        Net {
            id: NetId(id),
            terminals: terminals.iter().map(|&n| Terminal::at(NodeId(n))).collect(),
        }
    }

    fn point(node: u32, cost: u32) -> AccessPoint<NodeId> {
        AccessPoint {
            node: NodeId(node),
            cost: Cost::from(cost),
        }
    }

    fn terminal(points: &[(u32, u32)]) -> Terminal<NodeId> {
        Terminal::new(points.iter().map(|&(n, c)| point(n, c)).collect()).unwrap()
    }

    /// A directed graph with edges of the given costs.
    fn weighted_graph(nodes: u32, edges: &[(u32, u32, u32)]) -> Graph {
        let mut builder = GraphBuilder::new(nodes);
        for &(from, to, cost) in edges {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                .unwrap();
        }
        builder.build()
    }

    /// A directed graph with edges of cost 1.
    fn graph(nodes: u32, edges: &[(u32, u32)]) -> Graph {
        let edges: Vec<_> = edges.iter().map(|&(from, to)| (from, to, 1)).collect();
        weighted_graph(nodes, &edges)
    }

    fn options() -> EngineOptions<'static> {
        EngineOptions::default()
    }

    /// The nodes of a net's route, as plain numbers, in the route's order.
    fn nodes_of(outcome: &Outcome<NodeId, NodeId>, id: u32) -> Vec<u32> {
        let route = outcome.routes.route(NetId(id)).expect("net has a route");
        route.nodes().map(|node| node.0).collect()
    }

    /// Counts what each resource holds in `routes`, without any help from the engine: every net
    /// is a set of resources, gathered from its root and its edges.
    fn occupancy<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        routes: &RouteSet<S::Node>,
    ) -> BTreeMap<S::Resource, BTreeSet<NetId>> {
        let mut holders: BTreeMap<S::Resource, BTreeSet<NetId>> = BTreeMap::new();
        let mut out = Vec::new();
        for net in nets {
            let Some(route) = routes.route(net.id) else {
                continue;
            };
            out.clear();
            if let Some(root) = route.root() {
                space.resources(net.id, root, root, &mut out);
            }
            for edge in route.edges() {
                space.resources(net.id, edge.from, edge.to, &mut out);
            }
            for &resource in &out {
                holders.entry(resource).or_default().insert(net.id);
            }
        }
        holders
    }

    /// Checks each route on its own: it must be a tree grown from the driver over real steps,
    /// reaching every terminal. This walks the edges itself rather than using the checks in
    /// [`crate::net`], and it only looks at the problem and the routes.
    fn check_routes<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        routes: &RouteSet<S::Node>,
    ) -> Result<(), String> {
        for net in nets {
            let Some(outcome) = routes.outcome(net.id) else {
                return Err(format!("{:?} is not in the result", net.id));
            };
            let NetOutcome::Routed(route) = outcome else {
                continue;
            };
            let Some(root) = route.root() else {
                if net.needs_routing() {
                    return Err(format!("{:?} needs a route and has an empty one", net.id));
                }
                continue;
            };
            if !net.needs_routing() {
                return Err(format!("{:?} needs no route", net.id));
            }
            let driver = net.driver().unwrap().access_points();
            if !driver.iter().any(|point| point.node == root) {
                return Err(format!("{:?} is not rooted at its driver", net.id));
            }

            let mut on_tree = HashSet::from([root]);
            let mut steps = Vec::new();
            for edge in route.edges() {
                if !on_tree.contains(&edge.from) {
                    return Err(format!("{:?} has a disconnected {edge:?}", net.id));
                }
                if !on_tree.insert(edge.to) {
                    return Err(format!("{:?} has a loop at {edge:?}", net.id));
                }
                steps.clear();
                space.expand(net.id, edge.from, &mut steps);
                if !steps.iter().any(|step| step.to == edge.to) {
                    return Err(format!(
                        "{:?} takes a step that is not there: {edge:?}",
                        net.id
                    ));
                }
            }
            for (index, terminal) in net.terminals.iter().enumerate() {
                let points = terminal.access_points();
                if !points.iter().any(|point| on_tree.contains(&point.node)) {
                    return Err(format!("{:?} misses terminal {index}", net.id));
                }
            }
        }
        Ok(())
    }

    /// Checks that a result is complete and legal, using nothing from the engine.
    pub(super) fn check_legal<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        routes: &RouteSet<S::Node>,
    ) -> Result<(), String> {
        check_routes(space, nets, routes)?;
        if let Some((net, reason)) = routes.unrouted().next() {
            return Err(format!("{net:?} is unrouted: {reason}"));
        }
        for (resource, holders) in occupancy(space, nets, routes) {
            if holders.len() as u64 > u64::from(space.capacity(resource)) {
                return Err(format!("{resource:?} is overused by {holders:?}"));
            }
        }
        Ok(())
    }

    /// Checks that an outcome tells the truth about its routes, whatever it says they are.
    pub(super) fn check_honest<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        outcome: &Outcome<S::Node, S::Resource>,
    ) -> Result<(), String> {
        check_routes(space, nets, &outcome.routes)?;
        let expected: Vec<_> = occupancy(space, nets, &outcome.routes)
            .into_iter()
            .filter(|(resource, holders)| {
                holders.len() as u64 > u64::from(space.capacity(*resource))
            })
            .map(|(resource, holders)| {
                let capacity = space.capacity(resource);
                (resource, holders.len() as u32, capacity, holders)
            })
            .collect();
        let reported: Vec<_> = outcome
            .overused
            .iter()
            .map(|entry| {
                let nets: BTreeSet<NetId> = entry.nets.iter().copied().collect();
                (entry.resource, entry.occupancy, entry.capacity, nets)
            })
            .collect();
        if expected != reported {
            return Err(format!("overuse is {expected:?}, reported {reported:?}"));
        }
        if outcome.is_success() {
            check_legal(space, nets, &outcome.routes)?;
        }
        if outcome.is_success() != outcome.legal_routes().is_some() {
            return Err("is_success and legal_routes disagree".to_string());
        }
        Ok(())
    }

    /// Whether a driver access point exists that reaches every terminal, found by graph
    /// search over the unpriced space. This is what `NoPath` has to agree with.
    pub(super) fn routable<S: RoutingSpace>(space: &S, net: &Net<S::Node>) -> bool {
        if !net.needs_routing() {
            return true;
        }
        let mut steps = Vec::new();
        net.driver().unwrap().access_points().iter().any(|root| {
            let mut seen = HashSet::from([root.node]);
            let mut stack = vec![root.node];
            while let Some(node) = stack.pop() {
                steps.clear();
                space.expand(net.id, node, &mut steps);
                for step in &steps {
                    if seen.insert(step.to) {
                        stack.push(step.to);
                    }
                }
            }
            net.terminals[1..].iter().all(|terminal| {
                terminal
                    .access_points()
                    .iter()
                    .any(|point| seen.contains(&point.node))
            })
        })
    }

    /// Runs the engine with every check that applies to any result, and returns the outcome.
    fn route_checked<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        options: EngineOptions<'_>,
    ) -> Outcome<S::Node, S::Resource> {
        let outcome = route(space, nets, options).unwrap();
        if let Err(problem) = check_honest(space, nets, &outcome) {
            panic!("{problem}");
        }
        outcome
    }

    fn assert_legal<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
        outcome: &Outcome<S::Node, S::Resource>,
    ) where
        S::Node: Debug,
    {
        assert!(outcome.is_success(), "{:?}", outcome.overused);
        assert_eq!(outcome.stop, StopReason::Converged);
        if let Err(problem) = check_legal(space, nets, &outcome.routes) {
            panic!("{problem}");
        }
    }

    /// A random-order permutation of `0..n`, from a seed.
    fn permutation(n: u32, seed: u64) -> Vec<u32> {
        let mut items: Vec<u32> = (0..n).collect();
        crate::rng::Rng::new(seed).shuffle(&mut items);
        items
    }

    /// FNV-1a over 32-bit words, to pin routes to a value that any platform can compute. Each word
    /// is fed in as four little-endian bytes.
    fn hash_words(hash: &mut u64, word: u32) {
        for byte in word.to_le_bytes() {
            *hash ^= u64::from(byte);
            *hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    pub(super) fn hash_outcome(outcome: &Outcome<NodeId, NodeId>) -> u64 {
        let mut hash = 0xCBF2_9CE4_8422_2325;
        for (id, result) in outcome.routes.outcomes() {
            hash_words(&mut hash, id.0);
            match result {
                NetOutcome::Routed(route) => {
                    hash_words(&mut hash, route.edges().len() as u32);
                    hash_words(&mut hash, route.root().map_or(u32::MAX, |node| node.0));
                    for edge in route.edges() {
                        hash_words(&mut hash, edge.from.0);
                        hash_words(&mut hash, edge.to.0);
                    }
                }
                NetOutcome::Unrouted(_) => hash_words(&mut hash, u32::MAX),
            }
        }
        for entry in &outcome.overused {
            hash_words(&mut hash, entry.resource.0);
            hash_words(&mut hash, entry.occupancy);
        }
        hash
    }

    // Crossings and congestion.

    #[test]
    fn nets_that_share_nothing_finish_in_one_iteration() {
        let grid = Grid::new(4, 4, 1).unwrap();
        let nets = [net(0, &[0, 3]), net(1, &[12, 15])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(outcome.routes.stats.iterations, 1);
        assert_eq!(outcome.best_iteration, 1);
        assert_eq!(outcome.total_cost, Cost::from(6));
        assert_eq!(nodes_of(&outcome, 0), [0, 1, 2, 3]);
    }

    #[test]
    fn crossing_nets_resolve_with_a_layer_change() {
        // Both straight lines cross at the centre, so the first pass must conflict.
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];

        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        let first = engine.step().unwrap();
        assert_eq!(first.total_overuse, 1);
        assert_eq!(first.overused_resources, 1);
        assert_eq!(first.nets_rerouted, 2);
        let overused = engine.overused();
        assert_eq!(overused[0].resource, grid.node_at(2, 2, 0).unwrap());
        assert_eq!(overused[0].nets, [NetId(0), NetId(1)]);

        let outcome = engine.run();
        assert_legal(&grid, &nets, &outcome);
        assert!(outcome.routes.stats.iterations >= 2);
    }

    #[test]
    fn a_terminal_on_another_nets_shortest_path_pushes_that_net_away() {
        // One layer, so net 0 must go all the way around net 1, which cannot move off its
        // terminals. The obvious first pass runs net 0 straight through net 1's driver.
        let grid = Grid::new(4, 4, 1).unwrap();
        let nets = [net(0, &[0, 3]), net(1, &[1, 9])];
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        assert!(engine.step().unwrap().total_overuse > 0);
        let outcome = engine.run();
        assert_legal(&grid, &nets, &outcome);
        assert!(!nodes_of(&outcome, 0).contains(&1));
    }

    #[test]
    fn the_earlier_net_gives_way_when_only_the_later_net_has_no_choice() {
        // Net 0 prefers the bottleneck 2 but can use 3. Net 1 has only the bottleneck.
        let space = weighted_graph(
            7,
            &[
                (0, 2, 1),
                (2, 4, 1),
                (0, 3, 2),
                (3, 4, 2),
                (1, 2, 1),
                (2, 5, 1),
            ],
        );
        let nets = [net(0, &[0, 4]), net(1, &[1, 5])];
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [0, 3, 4]);
        assert_eq!(nodes_of(&outcome, 1), [1, 2, 5]);
        assert_eq!(outcome.total_cost, Cost::from(6));
    }

    #[test]
    fn zero_cost_steps_still_get_negotiated() {
        // A product with the base cost could never make these free steps dearer.
        let space = weighted_graph(
            6,
            &[
                (0, 2, 0),
                (2, 4, 0),
                (0, 3, 1),
                (3, 4, 0),
                (1, 2, 0),
                (2, 5, 0),
            ],
        );
        let nets = [net(0, &[0, 4]), net(1, &[1, 5])];
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
    }

    #[test]
    fn only_nets_on_overused_resources_are_routed_again() {
        let grid = Grid::new(5, 6, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            net(0, &[at(0, 2), at(4, 2)]),
            net(1, &[at(2, 0), at(2, 4)]),
            net(2, &[at(0, 5), at(4, 5)]), // Well away from the other two.
        ];
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().nets_rerouted, 3);
        let second = engine.step().unwrap();
        assert!(second.nets_rerouted <= 2);
        assert!(engine.states[2].cost == Cost::from(4));
    }

    #[test]
    fn a_conflict_settled_earlier_in_the_iteration_does_not_reroute_the_other_net() {
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        // A price high enough to make the first net to go move at once.
        let dear = EngineOptions {
            initial_present: Cost::from(10),
            ..options()
        };
        let mut engine = Engine::new(&grid, &nets, dear).unwrap();
        engine.step();
        // Net 0 moves off the shared cell, and that leaves net 1 with nothing to resolve.
        let second = engine.step().unwrap();
        assert_eq!(second.total_overuse, 0);
        assert_eq!(second.nets_rerouted, 1);
    }

    #[test]
    fn full_reroute_visits_every_net_every_time() {
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        let reroute_all = EngineOptions {
            reroute_all: true,
            ..options()
        };
        let mut engine = Engine::new(&grid, &nets, reroute_all).unwrap();
        while let Some(report) = engine.step() {
            assert_eq!(report.nets_rerouted, 2);
        }
        assert_legal(&grid, &nets, &engine.outcome());
    }

    // Capacity.

    #[test]
    fn capacity_above_one_is_shared_up_to_the_limit() {
        // Three nets through a middle node 3, which has room for all of them or not.
        let edges = [(0, 3), (1, 3), (2, 3), (3, 4), (3, 5), (3, 6)];
        let nets = [net(0, &[0, 4]), net(1, &[1, 5]), net(2, &[2, 6])];
        let with_capacity = |capacity| {
            let mut builder = GraphBuilder::new(7);
            for &(from, to) in &edges {
                builder
                    .add_edge(NodeId(from), NodeId(to), Cost::ONE)
                    .unwrap();
            }
            builder.set_capacity(NodeId(3), capacity).unwrap();
            builder.build()
        };

        let roomy = with_capacity(3);
        let outcome = route_checked(&roomy, &nets, options());
        assert_legal(&roomy, &nets, &outcome);
        assert_eq!(outcome.routes.stats.iterations, 1);

        let tight = with_capacity(2);
        let outcome = route_checked(&tight, &nets, options());
        assert!(!outcome.is_success());
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].occupancy, 3);
        assert_eq!(outcome.overused[0].capacity, 2);
    }

    #[test]
    fn capacity_two_channels_carry_two_nets_each() {
        // A corridor one cell wide with capacity 2, and four nets to send through two corridors.
        let mut grid = Grid::new(5, 3, 1).unwrap();
        for x in 1..4 {
            grid.set_capacity(x, 0, 0, 2).unwrap();
            grid.set_capacity(x, 2, 0, 2).unwrap();
            grid.set_blocked(x, 1, 0, true).unwrap();
        }
        for y in [0, 2] {
            grid.set_capacity(0, y, 0, 2).unwrap();
            grid.set_capacity(4, y, 0, 2).unwrap();
        }
        grid.set_capacity(0, 1, 0, 4).unwrap();
        grid.set_capacity(4, 1, 0, 4).unwrap();
        let from = grid.node_at(0, 1, 0).unwrap().0;
        let to = grid.node_at(4, 1, 0).unwrap().0;
        let nets: Vec<_> = (0..4).map(|id| net(id, &[from, to])).collect();
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
    }

    #[test]
    fn a_zero_capacity_resource_is_avoided_from_the_very_first_pass() {
        // The short way is through node 1, where any use is overuse. A search that assumed that
        // an untouched resource is free would need a second iteration to find that out.
        let mut builder = GraphBuilder::new(4);
        for (from, to, cost) in [(0, 1, 1), (1, 3, 1), (0, 2, 2), (2, 3, 2)] {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                .unwrap();
        }
        builder.set_capacity(NodeId(1), 0).unwrap();
        let space = builder.build();
        let nets = [net(0, &[0, 3])];

        // The first pass charges nothing for congestion, so it takes a price to see this.
        let priced_first = EngineOptions {
            first_present: Cost::from(10),
            ..options()
        };
        let outcome = route_checked(&space, &nets, priced_first);
        assert_legal(&space, &nets, &outcome);
        assert_eq!(outcome.routes.stats.iterations, 1);
        assert_eq!(nodes_of(&outcome, 0), [0, 2, 3]);

        // And by default it takes one more iteration, but gets there.
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [0, 2, 3]);
    }

    // Occupancy.

    /// A space whose steps consume a shared bus as well as the node they arrive at, which is how
    /// one net can consume a resource over and over.
    struct BusSpace {
        graph: Graph,
        bus_capacity: u32,
        reports_bus_twice: bool,
        root_resource: bool,
    }

    const BUS: NodeId = NodeId(1000);
    const ROOT_PREFIX: u32 = 2000;

    impl RoutingSpace for BusSpace {
        type Node = NodeId;
        type Resource = NodeId;
        type Slots = DenseSlots<NodeId>;
        type ResourceSlots = DenseSlots<NodeId>;

        fn new_slots(&self) -> Self::Slots {
            DenseSlots::new()
        }

        fn new_resource_slots(&self) -> Self::ResourceSlots {
            DenseSlots::new()
        }

        fn contains(&self, node: NodeId) -> bool {
            self.graph.contains(node)
        }

        fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
            self.graph.expand(net, from, out);
        }

        fn lower_bound(&self, _net: NetId, _from: NodeId, _to: NodeId) -> Cost {
            Cost::ZERO
        }

        fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
            if from == to {
                if self.root_resource {
                    out.push(NodeId(ROOT_PREFIX + to.0));
                }
                return;
            }
            out.push(BUS);
            out.push(to);
            if self.reports_bus_twice {
                out.push(BUS);
            }
            let _ = net;
        }

        fn capacity(&self, resource: NodeId) -> u32 {
            if resource == BUS {
                self.bus_capacity
            } else {
                1
            }
        }
    }

    #[test]
    fn a_net_counts_once_on_a_resource_used_by_many_edges() {
        for reports_bus_twice in [false, true] {
            let space = BusSpace {
                graph: graph(4, &[(0, 1), (1, 2), (2, 3)]),
                bus_capacity: 1,
                reports_bus_twice,
                root_resource: false,
            };
            let nets = [net(0, &[0, 3])];
            let outcome = route_checked(&space, &nets, options());
            assert_legal(&space, &nets, &outcome);
            assert_eq!(outcome.routes.stats.iterations, 1);
        }
    }

    #[test]
    fn two_nets_on_a_shared_bus_are_each_counted_once() {
        let space = BusSpace {
            graph: graph(6, &[(0, 1), (1, 2), (3, 4), (4, 5)]),
            bus_capacity: 1,
            reports_bus_twice: false,
            root_resource: false,
        };
        let nets = [net(0, &[0, 2]), net(1, &[3, 5])];
        let outcome = route_checked(&space, &nets, options());
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].resource, BUS);
        assert_eq!(outcome.overused[0].occupancy, 2);
        assert_eq!(outcome.overused[0].nets, [NetId(0), NetId(1)]);
    }

    #[test]
    fn a_growing_tree_is_not_charged_for_what_it_already_holds() {
        /// Steps consume a bus whose capacity is zero, except those on the long way through
        /// node 5. A net that holds the bus already should not pay to go on using it.
        struct OccupiedBus(Graph);
        impl RoutingSpace for OccupiedBus {
            type Node = NodeId;
            type Resource = NodeId;
            type Slots = DenseSlots<NodeId>;
            type ResourceSlots = DenseSlots<NodeId>;
            fn new_slots(&self) -> Self::Slots {
                DenseSlots::new()
            }
            fn new_resource_slots(&self) -> Self::ResourceSlots {
                DenseSlots::new()
            }
            fn contains(&self, node: NodeId) -> bool {
                self.0.contains(node)
            }
            fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
                self.0.expand(net, from, out);
            }
            fn lower_bound(&self, _: NetId, _: NodeId, _: NodeId) -> Cost {
                Cost::ZERO
            }
            fn resources(&self, _: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
                if from != to && from != NodeId(5) && to != NodeId(5) {
                    out.push(BUS);
                }
            }
            fn capacity(&self, _: NodeId) -> u32 {
                0
            }
        }

        // The first path has to take 0 -> 1, and so the bus. After that, 1 -> 2 costs 1 and
        // 1 -> 5 -> 2 costs 4, so a charge for the bus would send the net the long way.
        let inner = weighted_graph(6, &[(0, 1, 1), (1, 2, 1), (1, 5, 2), (5, 2, 2)]);
        let space = OccupiedBus(inner);
        let nets = [net(0, &[0, 1, 2])];
        let priced = EngineOptions {
            first_present: Cost::from(100),
            max_iterations: 1,
            ..options()
        };
        let outcome = route(&space, &nets, priced).unwrap();
        assert_eq!(nodes_of(&outcome, 0), [0, 1, 2]);
        assert_eq!(outcome.total_cost, Cost::from(2));
    }

    #[test]
    fn the_root_counts_as_occupied() {
        // Both nets start at node 0, which only one of them can hold. No edge consumes it, so
        // only the call with the root at both ends tells the engine.
        let space = graph(3, &[(0, 1), (0, 2)]);
        let nets = [net(0, &[0, 1]), net(1, &[0, 2])];
        let outcome = route_checked(&space, &nets, options());
        assert!(!outcome.is_success());
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].resource, NodeId(0));
        assert_eq!(outcome.overused[0].nets, [NetId(0), NetId(1)]);
    }

    #[test]
    fn a_congested_root_is_negotiated_away_when_the_driver_has_another() {
        // Net 1 must start at node 0, and net 0 can use either node 0 or the dearer node 1.
        let space = graph(4, &[(0, 2), (1, 2), (0, 3)]);
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![terminal(&[(0, 0), (1, 1)]), Terminal::at(NodeId(2))],
            },
            net(1, &[0, 3]),
        ];
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().total_overuse, 1);
        let outcome = engine.run();
        assert_legal(&space, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [1, 2]);
        assert_eq!(outcome.total_cost, Cost::from(3));
    }

    // Unroutable nets and impossible problems.

    #[test]
    fn a_net_with_no_path_is_reported_and_others_still_route() {
        let grid = {
            let mut grid = Grid::new(5, 5, 1).unwrap();
            for y in 0..5 {
                grid.set_blocked(2, y, 0, true).unwrap();
            }
            grid
        };
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            net(0, &[at(0, 0), at(1, 4)]),
            net(1, &[at(0, 2), at(4, 2)]), // Across the wall.
            net(2, &[at(3, 0), at(4, 4)]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert!(!outcome.is_success());
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(1), UnroutedReason::NoPath)]
        );
        assert_eq!(outcome.routes.routes().count(), 2);
        assert!(outcome.overused.is_empty());
        assert_eq!(outcome.stop, StopReason::Converged);
        assert!(outcome.legal_routes().is_none());
    }

    #[test]
    fn a_net_with_no_path_is_searched_for_once() {
        // Nets 1 and 2 conflict for a while, and that must not repeat net 0's failed search,
        // which has to flood everything it can reach to prove anything.
        let space = graph(8, &[(0, 1), (2, 4), (3, 4), (4, 5), (4, 6), (2, 7), (7, 5)]);
        let nets = [net(0, &[0, 6]), net(1, &[2, 5]), net(2, &[3, 6])];
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().searches, 3);
        while let Some(report) = engine.step() {
            assert!(report.searches <= 2);
        }
    }

    #[test]
    fn a_directed_dead_end_tells_a_blocked_way_from_a_bad_root() {
        // The driver has two access points. From node 0, the near terminal is 2 but the far one
        // cannot be reached, whereas node 1 reaches both, at a higher cost. A router that keeps
        // to its first choice of root would wrongly report no path.
        let space = weighted_graph(5, &[(0, 2, 1), (1, 2, 5), (1, 3, 5)]);
        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                terminal(&[(0, 0), (1, 0)]),
                Terminal::at(NodeId(2)),
                Terminal::at(NodeId(3)),
            ],
        }];
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        assert_eq!(engine.step().unwrap().searches, 4);
        assert_eq!(engine.states[0].dead_roots, [NodeId(0)]);
        let outcome = engine.run();
        assert_legal(&space, &nets, &outcome);
        assert_eq!(
            outcome.routes.route(NetId(0)).unwrap().root(),
            Some(NodeId(1))
        );
    }

    #[test]
    fn no_root_reaching_everything_is_no_path_even_if_each_terminal_is_reachable() {
        // Each root reaches one terminal, and neither reaches both.
        let space = graph(4, &[(0, 2), (1, 3)]);
        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                terminal(&[(0, 0), (1, 0)]),
                Terminal::at(NodeId(2)),
                Terminal::at(NodeId(3)),
            ],
        }];
        let outcome = route_checked(&space, &nets, options());
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(0), UnroutedReason::NoPath)]
        );
        assert_eq!(outcome.total_cost, Cost::ZERO);
    }

    #[test]
    fn too_little_capacity_terminates_and_says_so() {
        // Three nets through one node, on a problem with no other way across.
        let space = graph(7, &[(0, 3), (1, 3), (2, 3), (3, 4), (3, 5), (3, 6)]);
        let nets = [net(0, &[0, 4]), net(1, &[1, 5]), net(2, &[2, 6])];
        let limited = EngineOptions {
            max_iterations: 20,
            ..options()
        };
        let outcome = route_checked(&space, &nets, limited);

        assert_eq!(outcome.stop, StopReason::IterationLimit);
        assert_eq!(outcome.routes.stats.iterations, 20);
        assert!(!outcome.is_success());
        assert!(outcome.legal_routes().is_none());
        assert_eq!(outcome.routes.routes().count(), 3);
        assert_eq!(outcome.overused.len(), 1);
        assert_eq!(outcome.overused[0].resource, NodeId(3));
        assert_eq!(outcome.overused[0].occupancy, 3);
        assert_eq!(outcome.overused[0].capacity, 1);
        assert_eq!(outcome.conflicted_nets(), [NetId(0), NetId(1), NetId(2)]);
    }

    #[test]
    fn too_little_capacity_on_a_grid_keeps_the_best_iteration() {
        // A wall with one gap, and more nets than the gap can take. They stay in conflict
        // whatever happens, with the best iteration being the first of the least overuse.
        let mut grid = Grid::new(5, 6, 1).unwrap();
        for y in 0..6 {
            if y != 3 {
                grid.set_blocked(2, y, 0, true).unwrap();
            }
        }
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets: Vec<_> = (0..4).map(|y| net(y, &[at(0, y), at(4, y)])).collect();
        let limited = EngineOptions {
            max_iterations: 30,
            ..options()
        };
        let mut engine = Engine::new(&grid, &nets, limited).unwrap();
        while engine.step().is_some() {}
        let outcome = engine.outcome();
        assert!(check_honest(&grid, &nets, &outcome).is_ok());
        assert!(!outcome.is_success());
        assert_eq!(outcome.stop, StopReason::IterationLimit);

        // The ordering is exactly as documented.
        let reports = engine.reports();
        let rank = |r: &IterationReport| (r.unrouted, r.total_overuse, r.total_cost);
        let best = reports.iter().map(rank).min().unwrap();
        let first_best = reports.iter().position(|r| rank(r) == best).unwrap();
        assert_eq!(outcome.best_iteration, first_best as u32 + 1);
        assert_eq!(outcome.total_cost, best.2);
    }

    #[test]
    fn best_prefers_fewer_unrouted_over_less_overuse() {
        let (best, candidate) = ((1u32, 0u64, Cost::ONE), (0u32, 9u64, Cost::MAX));
        assert!(candidate < best);
    }

    // Multi-terminal nets.

    #[test]
    fn a_tree_grows_to_each_terminal_with_shared_trunk() {
        // A corridor along the middle row, with one stub up and one stub down.
        let mut grid = Grid::new(7, 3, 1).unwrap();
        for x in 0..7 {
            if x != 3 {
                grid.set_blocked(x, 0, 0, true).unwrap();
            }
            if x != 5 {
                grid.set_blocked(x, 2, 0, true).unwrap();
            }
        }
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 1), at(6, 1), at(3, 0), at(5, 2)])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        // Six along the corridor, shared by all three paths, and the two stubs.
        assert_eq!(outcome.total_cost, Cost::from(8));
        assert_eq!(outcome.routes.route(NetId(0)).unwrap().edges().len(), 8);
    }

    #[test]
    fn a_path_passing_over_a_terminal_connects_it_without_a_search() {
        let grid = Grid::new(5, 1, 1).unwrap();
        let nets = [net(0, &[0, 4, 2])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        // The nearest terminal is 2, then a search from the tree finishes at 4.
        assert_eq!(outcome.routes.stats.searches, 2);

        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                Terminal::at(NodeId(0)),
                terminal(&[(4, 0)]),
                terminal(&[(2, 100)]),
            ],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        // A long way to 4 passes over 2 for free, leaving nothing else to do.
        assert_eq!(outcome.routes.stats.searches, 1);
        assert_eq!(outcome.total_cost, Cost::from(104));
    }

    #[test]
    fn terminals_on_one_node_need_nothing_but_a_root() {
        let grid = Grid::new(3, 1, 1).unwrap();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![
                terminal(&[(0, 1), (1, 0)]),
                terminal(&[(1, 0), (2, 0)]),
                terminal(&[(1, 2)]),
            ],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(
            outcome.routes.route(NetId(0)),
            Some(&Route::from_root(NodeId(1)))
        );
        assert_eq!(outcome.total_cost, Cost::from(2));
    }

    #[test]
    fn single_and_empty_nets_get_empty_routes_and_occupy_nothing() {
        let grid = Grid::new(3, 1, 1).unwrap();
        let nets = [
            net(0, &[1]),
            Net {
                id: NetId(1),
                terminals: vec![],
            },
            net(2, &[0, 2]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(outcome.routes.route(NetId(0)), Some(&Route::new()));
        assert_eq!(outcome.routes.route(NetId(1)), Some(&Route::new()));
        assert_eq!(outcome.routes.stats.searches, 1);
    }

    #[test]
    fn multi_terminal_nets_negotiate_like_any_other() {
        let grid = Grid::new(6, 6, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            net(0, &[at(0, 0), at(5, 5), at(0, 5)]),
            net(1, &[at(5, 0), at(0, 2), at(3, 5)]),
            net(2, &[at(2, 0), at(2, 5), at(5, 3)]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
    }

    // Equivalent access points.

    #[test]
    fn any_access_point_of_a_terminal_satisfies_it() {
        let grid = Grid::new(6, 1, 1).unwrap();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![Terminal::at(NodeId(2)), terminal(&[(5, 0), (3, 0)])],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [2, 3]);
    }

    #[test]
    fn access_costs_steer_the_choice_of_access_point() {
        let grid = Grid::new(10, 1, 1).unwrap();
        let nets = [Net {
            id: NetId(0),
            terminals: vec![Terminal::at(NodeId(2)), terminal(&[(3, 9), (7, 0)])],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_eq!(nodes_of(&outcome, 0), [2, 3, 4, 5, 6, 7]);
        assert_eq!(outcome.total_cost, Cost::from(5));

        // At the driver too: starting further away costs less.
        let nets = [Net {
            id: NetId(0),
            terminals: vec![terminal(&[(2, 9), (8, 1)]), Terminal::at(NodeId(5))],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_eq!(nodes_of(&outcome, 0), [8, 7, 6, 5]);
        assert_eq!(outcome.total_cost, Cost::from(4));
    }

    #[test]
    fn congestion_moves_a_terminal_to_another_access_point() {
        // Net 0's nearer access point at 2 lies on net 1's only way.
        let grid = Grid::new(5, 3, 1).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![
                    Terminal::at(NodeId(at(0, 1))),
                    terminal(&[(at(1, 1), 0), (at(0, 0), 3)]),
                ],
            },
            net(1, &[at(1, 0), at(1, 2)]),
        ];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [at(0, 1), at(0, 0)]);
    }

    // Budgets and cancellation.

    #[test]
    fn a_cancel_flag_already_set_stops_before_any_work() {
        let grid = Grid::new(4, 4, 1).unwrap();
        let flag = AtomicBool::new(true);
        let nets = [net(0, &[0, 3]), net(1, &[5]), net(2, &[0, 15])];
        let cancelled = EngineOptions {
            cancel: Some(&flag),
            ..options()
        };

        let mut engine = Engine::new(&grid, &nets, cancelled).unwrap();
        assert_eq!(engine.step(), None);
        assert_eq!(engine.stop_reason(), Some(StopReason::Cancelled));
        let outcome = engine.outcome();

        assert_eq!(outcome.stop, StopReason::Cancelled);
        assert_eq!(outcome.routes.stats, RouteStats::default());
        assert_eq!(outcome.best_iteration, 0);
        assert!(!outcome.is_success());
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [
                (NetId(0), UnroutedReason::Cancelled),
                (NetId(2), UnroutedReason::Cancelled)
            ]
        );
        // Net 1 needs no routing, so that is no failure.
        assert_eq!(outcome.routes.route(NetId(1)), Some(&Route::new()));
    }

    /// A grid that raises a flag once a given number of expansions have gone by.
    struct Tripwire<'a> {
        grid: Grid,
        flag: &'a AtomicBool,
        left: Cell<u32>,
    }

    impl RoutingSpace for Tripwire<'_> {
        type Node = NodeId;
        type Resource = NodeId;
        type Slots = DenseSlots<NodeId>;
        type ResourceSlots = DenseSlots<NodeId>;

        fn new_slots(&self) -> Self::Slots {
            DenseSlots::new()
        }

        fn new_resource_slots(&self) -> Self::ResourceSlots {
            DenseSlots::new()
        }

        fn contains(&self, node: NodeId) -> bool {
            self.grid.contains(node)
        }

        fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
            if self.left.get() == 0 {
                self.flag.store(true, Ordering::Relaxed);
            } else {
                self.left.set(self.left.get() - 1);
            }
            self.grid.expand(net, from, out);
        }

        fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
            self.grid.lower_bound(net, from, to)
        }

        fn resources(&self, net: NetId, from: NodeId, to: NodeId, out: &mut Vec<NodeId>) {
            self.grid.resources(net, from, to, out);
        }

        fn capacity(&self, resource: NodeId) -> u32 {
            self.grid.capacity(resource)
        }
    }

    #[test]
    fn cancelling_part_way_keeps_routes_and_the_best_so_far() {
        let flag = AtomicBool::new(false);
        let space = Tripwire {
            grid: Grid::new(5, 5, 2).unwrap(),
            flag: &flag,
            left: Cell::new(u32::MAX),
        };
        let at = |x, y| space.grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        let cancelled = EngineOptions {
            cancel: Some(&flag),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, cancelled).unwrap();
        let first = engine.step().unwrap();
        assert_eq!(first.total_overuse, 1);

        // The flag goes up during the next search, which is too short to notice. So the
        // search finishes and the iteration stops before the following net.
        space.left.set(0);
        let second = engine.step().unwrap();
        assert!(!second.completed);
        assert_eq!(engine.stop_reason(), Some(StopReason::Cancelled));
        assert_eq!(engine.step(), None);

        // Every net still has its route from before, including the one interrupted.
        let outcome = engine.outcome();
        assert_eq!(outcome.stop, StopReason::Cancelled);
        assert_eq!(outcome.routes.routes().count(), 2);
        assert_eq!(outcome.overused.len(), 1);
        assert!(check_honest(&space, &nets, &outcome).is_ok());
        assert!(engine.table.states.iter().map(|s| s.occupancy).sum::<u32>() > 0);
        let held: u32 = engine.states.iter().map(|s| s.held.len() as u32).sum();
        assert_eq!(
            engine.table.states.iter().map(|s| s.occupancy).sum::<u32>(),
            held
        );
    }

    #[test]
    fn cancelling_during_the_first_iteration_says_which_nets_it_cut_off() {
        let flag = AtomicBool::new(false);
        let space = Tripwire {
            grid: Grid::new(6, 2, 1).unwrap(),
            flag: &flag,
            left: Cell::new(6),
        };
        let nets = [net(0, &[0, 5]), net(1, &[6, 11])];
        let cancelled = EngineOptions {
            cancel: Some(&flag),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, cancelled).unwrap();
        let report = engine.step().unwrap();
        assert!(!report.completed);
        let outcome = engine.outcome();
        assert_eq!(outcome.stop, StopReason::Cancelled);
        assert_eq!(outcome.routes.route(NetId(0)).unwrap().edges().len(), 5);
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(1), UnroutedReason::Cancelled)]
        );
        assert!(!outcome.is_success());
    }

    #[test]
    fn a_budget_of_nothing_is_not_mistaken_for_no_path() {
        let grid = Grid::new(4, 4, 1).unwrap();
        let nets = [net(0, &[0, 3]), net(1, &[5]), net(2, &[6, 6])];
        let budget = EngineOptions {
            max_expansions: 0,
            ..options()
        };
        let outcome = route_checked(&grid, &nets, budget);
        assert_eq!(outcome.stop, StopReason::BudgetExhausted);
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [
                (NetId(0), UnroutedReason::BudgetExhausted),
                (NetId(2), UnroutedReason::BudgetExhausted)
            ]
        );
    }

    #[test]
    fn a_budget_stops_at_the_limit_and_reports_truthfully() {
        let grid = Grid::new(10, 10, 3).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets: Vec<_> = (0..10).map(|y| net(y, &[at(0, y), at(9, 9 - y)])).collect();
        let full = route_checked(&grid, &nets, options());
        let needed = full.routes.stats.nodes_expanded;
        assert!(full.is_success());

        // Exactly what was needed is enough, and gives the same result.
        let exact = EngineOptions {
            max_expansions: needed,
            ..options()
        };
        assert_eq!(route_checked(&grid, &nets, exact), full);

        // With less, it ends with an honest result that is never over budget.
        for divisor in [2, 3, 7, 50, 1000] {
            let budget = needed / divisor;
            let limited = EngineOptions {
                max_expansions: budget,
                ..options()
            };
            let outcome = route_checked(&grid, &nets, limited);
            assert_eq!(outcome.stop, StopReason::BudgetExhausted);
            assert!(outcome.routes.stats.nodes_expanded <= budget);
            assert!(
                outcome
                    .routes
                    .unrouted()
                    .all(|(_, reason)| reason == UnroutedReason::BudgetExhausted)
            );
        }
    }

    #[test]
    fn an_iteration_limit_of_zero_routes_nothing_and_says_why() {
        let grid = Grid::new(4, 1, 1).unwrap();
        let nets = [net(0, &[0, 3])];
        let none = EngineOptions {
            max_iterations: 0,
            ..options()
        };
        let outcome = route_checked(&grid, &nets, none);
        assert_eq!(outcome.stop, StopReason::IterationLimit);
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(0), UnroutedReason::BudgetExhausted)]
        );
    }

    // Stepping.

    #[test]
    fn stepping_matches_running_and_reports_each_iteration() {
        let grid = Grid::new(8, 8, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets: Vec<_> = (0..8).map(|y| net(y, &[at(0, y), at(7, 7 - y)])).collect();

        let run = route(&grid, &nets, options()).unwrap();

        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        assert_eq!(engine.stop_reason(), None);
        assert_eq!(engine.outcome().stop, StopReason::NotFinished);
        let mut count = 0;
        let (mut searches, mut expanded) = (0, 0);
        while let Some(report) = engine.step() {
            count += 1;
            assert_eq!(report.iteration, count);
            assert!(report.completed);
            searches += report.searches;
            expanded += report.nodes_expanded;
            // Observable, and consistent with what the report says.
            let overused = engine.overused();
            assert_eq!(overused.len() as u32, report.overused_resources);
            let total: u64 = overused
                .iter()
                .map(|entry| u64::from(entry.occupancy - entry.capacity))
                .sum();
            assert_eq!(total, report.total_overuse);
            assert_eq!(report.unrouted, 0);
        }
        assert_eq!(engine.step(), None);
        assert_eq!(engine.reports().len() as u32, count);

        let stepped = engine.outcome();
        assert_eq!(stepped, run);
        assert_eq!(stepped.routes.stats.iterations, u64::from(count));
        assert_eq!(stepped.routes.stats.searches, searches);
        assert_eq!(stepped.routes.stats.nodes_expanded, expanded);
    }

    #[test]
    fn stopping_between_iterations_keeps_the_best_so_far() {
        let grid = Grid::new(5, 5, 2).unwrap();
        let at = |x, y| grid.node_at(x, y, 0).unwrap().0;
        let nets = [net(0, &[at(0, 2), at(4, 2)]), net(1, &[at(2, 0), at(2, 4)])];
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        engine.step();
        let outcome = engine.outcome();
        assert_eq!(outcome.stop, StopReason::NotFinished);
        assert_eq!(outcome.best_iteration, 1);
        assert_eq!(outcome.routes.routes().count(), 2);
        assert_eq!(outcome.overused.len(), 1);
        assert!(outcome.legal_routes().is_none());
        assert!(check_honest(&grid, &nets, &outcome).is_ok());
    }

    #[test]
    fn the_present_factor_follows_its_schedule_up_to_the_cap() {
        let space = graph(4, &[(0, 2), (1, 2), (2, 3)]);
        let nets = [net(0, &[0, 3]), net(1, &[1, 3])];
        let schedule = EngineOptions {
            max_iterations: 8,
            first_present: Cost::ZERO,
            initial_present: Cost::from(2),
            present_growth: Cost::from(3),
            max_present: Cost::from(100),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, schedule).unwrap();
        let mut seen = Vec::new();
        while let Some(report) = engine.step() {
            seen.push(report.present.get());
        }
        assert_eq!(seen, [0.0, 2.0, 6.0, 18.0, 54.0, 100.0, 100.0, 100.0]);
    }

    #[test]
    fn history_accumulates_only_on_overused_resources() {
        // Node 2 is overused by one, with capacity two and three nets on it, until the end.
        let mut builder = GraphBuilder::new(7);
        for (from, to) in [(0, 3), (1, 3), (2, 3), (3, 4), (3, 5), (3, 6)] {
            builder
                .add_edge(NodeId(from), NodeId(to), Cost::ONE)
                .unwrap();
        }
        builder.set_capacity(NodeId(3), 2).unwrap();
        let space = builder.build();
        let nets = [net(0, &[0, 4]), net(1, &[1, 5]), net(2, &[2, 6])];
        let schedule = EngineOptions {
            max_iterations: 3,
            history_factor: Cost::from(5),
            ..options()
        };
        let mut engine = Engine::new(&space, &nets, schedule).unwrap();
        engine.step();
        engine.step();
        let history = |engine: &Engine<'_, Graph>, node| engine.table.states[node as usize].history;
        assert_eq!(history(&engine, 3), Cost::from(10));
        assert_eq!(history(&engine, 4), Cost::ZERO);
        // None is added for the iteration that ends the run, as nothing would ever read it.
        engine.step();
        assert_eq!(engine.stop_reason(), Some(StopReason::IterationLimit));
        assert_eq!(history(&engine, 3), Cost::from(10));
    }

    // The pricing wrapper.

    #[test]
    fn prices_follow_the_formula() {
        let space = BusSpace {
            graph: graph(4, &[(0, 1), (1, 2), (0, 2)]),
            bus_capacity: 2,
            reports_bus_twice: false,
            root_resource: false,
        };
        let mut table = ResourceTable::new(&space);
        let bus = table.intern(&space, BUS);
        let two = table.intern(&space, NodeId(2));
        table.states[bus].occupancy = 3;
        table.states[bus].history = Cost::from(2);
        table.states[two].occupancy = 1;
        table.states[two].history = Cost::from(5);

        let held = FastSet::default();
        let buffer = RefCell::new(Vec::new());
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
            replacing: &FastSet::default(),
            tied: None,
        };

        let mut out = vec![Step {
            to: NodeId(77),
            cost: Cost::ONE,
        }];
        priced.expand(NetId(0), NodeId(0), &mut out);
        // Entries already there are left alone.
        assert_eq!(out[0].cost, Cost::ONE);
        // To node 1: the bus alone is in the table, at occupancy 3 of 2 and with history 2, so
        // (1 + 2) * (1 + 10 * 2). A resource missing from the table adds nothing.
        assert_eq!(out[1].to, NodeId(1));
        assert_eq!(out[1].cost, Cost::from(63));
        // To node 2: the bus and node 2 add up. Overuse 2 + 1, history 2 + 5.
        assert_eq!(out[2].to, NodeId(2));
        assert_eq!(out[2].cost, Cost::from(8 * 31));

        // What the net already holds is priced as if it were not there.
        let mut held = FastSet::default();
        held.insert(BUS);
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
            replacing: &FastSet::default(),
            tied: None,
        };
        let mut out = Vec::new();
        priced.expand(NetId(0), NodeId(0), &mut out);
        assert_eq!(out[0].cost, Cost::ONE);
        assert_eq!(out[1].cost, Cost::from(6 * 11));
    }

    #[test]
    fn a_resource_listed_twice_for_one_step_is_priced_once() {
        let space = BusSpace {
            graph: graph(2, &[(0, 1)]),
            bus_capacity: 1,
            reports_bus_twice: true,
            root_resource: false,
        };
        let mut table = ResourceTable::new(&space);
        let bus = table.intern(&space, BUS);
        table.states[bus].occupancy = 1;
        table.states[bus].history = Cost::from(2);
        let held = FastSet::default();
        let buffer = RefCell::new(Vec::new());
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
            replacing: &FastSet::default(),
            tied: None,
        };
        let mut out = Vec::new();
        priced.expand(NetId(0), NodeId(0), &mut out);
        // Overuse 1 and history 2, and not twice either of them.
        assert_eq!(out[0].cost, Cost::from(3 * 11));
    }

    #[test]
    fn prices_never_fall_below_the_base_cost() {
        let values = [0, 1, 2, 7, 1000];
        let space = graph(1, &[]);
        for base in values {
            for history in values {
                for occupancy in values {
                    for present in values {
                        let mut table = ResourceTable::new(&space);
                        let entry = table.intern(&space, NodeId(0));
                        table.states[entry].history = Cost::from(history);
                        table.states[entry].occupancy = occupancy;
                        let held = FastSet::default();
                        let buffer = RefCell::new(Vec::new());
                        let priced = Priced {
                            inner: &space,
                            table: &table,
                            present: Cost::from(present),
                            held: &held,
                            buffer: &buffer,
                            replacing: &FastSet::default(),
                            tied: None,
                        };
                        let price = priced.price(Cost::from(base), &mut vec![NodeId(0)]);
                        assert!(price >= Cost::from(base));
                    }
                }
            }
        }
    }

    /// The prices of the steps out of `(0, 0)` of a space whose resources go through interned
    /// slots.
    fn lazy_pricing(table: &ResourceTable<Lazy>, space: &Lazy, present: u32) -> Vec<f64> {
        let held = FastSet::default();
        let buffer = RefCell::new(Vec::new());
        let priced = Priced {
            inner: space,
            table,
            present: Cost::from(present),
            held: &held,
            buffer: &buffer,
            replacing: &FastSet::default(),
            tied: None,
        };
        let mut out = Vec::new();
        priced.expand(NetId(0), (0, 0), &mut out);
        out.iter().map(|step| step.cost.get()).collect()
    }

    #[test]
    fn a_resource_that_no_net_has_occupied_is_priced_by_its_capacity() {
        // Slots 1, 2 and 3 exist in the arrays only because slot 4 made them grow, and none
        // of them has been told its capacity. Reading them as a capacity of 0 would overcharge
        // node 3, and reading them as free would undercharge node 2.
        let mut builder = GraphBuilder::new(5);
        for node in 1..5 {
            builder
                .add_edge(NodeId(0), NodeId(node), Cost::ONE)
                .unwrap();
        }
        builder.set_capacity(NodeId(2), 0).unwrap();
        builder.set_capacity(NodeId(4), 0).unwrap();
        let space = builder.build();

        let mut table = ResourceTable::new(&space);
        let far = table.intern(&space, NodeId(4));
        table.acquire(&[far]);
        assert_eq!(table.states.len(), 5);

        let held = FastSet::default();
        let buffer = RefCell::new(Vec::new());
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
            replacing: &FastSet::default(),
            tied: None,
        };
        let mut out = Vec::new();
        priced.expand(NetId(0), NodeId(0), &mut out);
        let prices: Vec<f64> = out.iter().map(|step| step.cost.get()).collect();
        // Free. No capacity, so overuse 1. Free. Occupied with no capacity, so overuse 2.
        assert_eq!(prices, [1.0, 11.0, 1.0, 21.0]);

        // The same goes for a slot past the end of the arrays.
        let mut table = ResourceTable::new(&space);
        let near = table.intern(&space, NodeId(1));
        table.acquire(&[near]);
        assert_eq!(table.states.len(), 2);
        let priced = Priced {
            inner: &space,
            table: &table,
            present: Cost::from(10),
            held: &held,
            buffer: &buffer,
            replacing: &FastSet::default(),
            tied: None,
        };
        let mut out = Vec::new();
        priced.expand(NetId(0), NodeId(0), &mut out);
        let prices: Vec<f64> = out.iter().map(|step| step.cost.get()).collect();
        assert_eq!(prices, [11.0, 11.0, 1.0, 11.0]);
    }

    #[test]
    fn a_resource_given_up_again_is_priced_like_one_never_occupied() {
        let space = graph(2, &[(0, 1)]);
        let mut table = ResourceTable::new(&space);
        let slot = table.intern(&space, NodeId(1));
        table.acquire(&[slot]);
        table.release(&[slot]);
        assert!(table.read(NodeId(1)).is_none());
        // And it is listed once, however many times it comes and goes.
        assert_eq!(table.intern(&space, NodeId(1)), slot);
        table.acquire(&[slot]);
        assert_eq!(table.occupied_slots, [slot]);
        assert_eq!(table.occupied_resources, [NodeId(1)]);
        assert_eq!(table.overuse(), (0, 0, 0));
        table.acquire(&[slot]);
        assert_eq!(table.overuse(), (1, 1, 0));
    }

    #[test]
    fn a_missing_interned_resource_is_free_and_priced_by_its_capacity() {
        let space = Lazy { size: 3 };
        let mut table = ResourceTable::new(&space);
        // Unassigned, and so read without touching anything.
        assert_eq!(lazy_pricing(&table, &space, 10), [1.0, 1.0]);
        assert_eq!(table.slots.get((1, 0)), None);
        assert!(table.states.is_empty());

        let slot = table.intern(&space, (1, 0));
        table.acquire(&[slot]);
        assert_eq!(lazy_pricing(&table, &space, 10), [11.0, 1.0]);
        table.states[slot].history = Cost::from(2);
        assert_eq!(lazy_pricing(&table, &space, 10), [33.0, 1.0]);
    }

    #[test]
    fn the_table_can_be_read_from_several_threads() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<ResourceTable<Grid>>();
        assert_sync::<ResourceTable<Graph>>();
        assert_sync::<ResourceTable<Lazy>>();
    }

    #[test]
    fn dense_and_interned_resources_negotiate_to_the_same_routes() {
        /// A grid with its resources relabelled, so that they go through a hash map and get
        /// numbered as they are first occupied, rather than by cell.
        struct Relabelled(Grid);
        impl RoutingSpace for Relabelled {
            type Node = NodeId;
            type Resource = (u32, u32);
            type Slots = DenseSlots<NodeId>;
            type ResourceSlots = InternedSlots<(u32, u32)>;
            fn new_slots(&self) -> Self::Slots {
                DenseSlots::new()
            }
            fn new_resource_slots(&self) -> Self::ResourceSlots {
                InternedSlots::new()
            }
            fn contains(&self, node: NodeId) -> bool {
                self.0.contains(node)
            }
            fn expand(&self, net: NetId, from: NodeId, out: &mut Vec<Step<NodeId>>) {
                self.0.expand(net, from, out);
            }
            fn lower_bound(&self, net: NetId, from: NodeId, to: NodeId) -> Cost {
                self.0.lower_bound(net, from, to)
            }
            fn resources(&self, _: NetId, _: NodeId, to: NodeId, out: &mut Vec<(u32, u32)>) {
                if self.0.contains(to) {
                    // The cell number is kept as the first field, so resources sort the same.
                    out.push((to.0, 7));
                }
            }
            fn capacity(&self, (cell, _): (u32, u32)) -> u32 {
                self.0.capacity(NodeId(cell))
            }
        }

        for seed in 0..4 {
            let (grid, nets) = permutation_problem(14, 2, seed);
            let mut dense = Engine::new(&grid, &nets, options()).unwrap();
            let relabelled = Relabelled(grid.clone());
            let mut interned = Engine::new(&relabelled, &nets, options()).unwrap();
            while dense.step().is_some() {}
            while interned.step().is_some() {}
            assert_eq!(dense.reports(), interned.reports());
            let (dense, interned) = (dense.outcome(), interned.outcome());
            assert_eq!(dense.routes, interned.routes);
            assert_eq!(dense.total_cost, interned.total_cost);
            assert_eq!(dense.overused.len(), interned.overused.len());
            for (a, b) in dense.overused.iter().zip(&interned.overused) {
                assert_eq!((a.resource.0, a.occupancy), (b.resource.0, b.occupancy));
                assert_eq!(a.nets, b.nets);
            }
        }
    }

    // Nodes outside the space.

    #[test]
    fn a_terminal_outside_the_space_is_no_path_for_that_net_alone() {
        let far = u32::MAX;
        let grid = Grid::new(4, 4, 1).unwrap();
        let space = graph(3, &[(0, 1), (1, 2)]);
        for nets in [
            // As a driver, as another terminal, and as the only terminal.
            [net(0, &[far, 1]), net(1, &[1, far]), net(2, &[far])],
            [net(0, &[far, far]), net(1, &[0, 1, far]), net(2, &[far])],
        ] {
            let mut all = nets.to_vec();
            all.push(net(3, &[0, 2]));

            let outcome = route(&grid, &all, options()).unwrap();
            for id in 0..3 {
                assert_eq!(
                    outcome.routes.outcome(NetId(id)),
                    Some(&NetOutcome::Unrouted(UnroutedReason::NoPath)),
                );
            }
            assert_eq!(nodes_of(&outcome, 3), [0, 1, 2]);
            assert_eq!(outcome.stop, StopReason::Converged);
            assert!(!outcome.is_success());
            assert!(outcome.overused.is_empty());
            // None of those nets got as far as a search.
            assert_eq!(outcome.routes.stats.searches, 1);

            let outcome = route(&space, &all, options()).unwrap();
            assert_eq!(outcome.routes.unrouted().count(), 3);
            assert_eq!(nodes_of(&outcome, 3), [0, 1, 2]);
        }
    }

    #[test]
    fn nets_outside_the_space_are_counted_in_reports_as_unrouted() {
        let grid = Grid::new(4, 1, 1).unwrap();
        let nets = [net(0, &[100]), net(1, &[100, 0]), net(2, &[0, 3])];
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        let report = engine.step().unwrap();
        assert_eq!(report.unrouted, 2);
        assert_eq!(report.nets_rerouted, 1);
        assert_eq!(engine.step(), None);
    }

    #[test]
    fn access_points_outside_the_space_are_ignored_when_others_are_inside() {
        let far = u32::MAX;
        let grid = Grid::new(4, 1, 1).unwrap();
        let nets = [Net {
            id: NetId(0),
            // The foreign access points are the cheapest, and neither is any use.
            terminals: vec![
                terminal(&[(far, 0), (0, 5)]),
                terminal(&[(far - 1, 0), (3, 7)]),
            ],
        }];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(nodes_of(&outcome, 0), [0, 1, 2, 3]);
        assert_eq!(outcome.total_cost, Cost::from(3 + 5 + 7));
    }

    #[test]
    fn a_single_terminal_inside_the_space_still_gets_an_empty_route() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let nets = [net(0, &[1])];
        let outcome = route_checked(&grid, &nets, options());
        assert!(outcome.is_success());
        assert_eq!(nodes_of(&outcome, 0), [] as [u32; 0]);
    }

    #[test]
    fn terminals_outside_a_generated_space_do_not_get_slots() {
        let space = Lazy { size: 3 };
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![Terminal::at((-1, 0)), Terminal::at((1, 0))],
            },
            Net {
                id: NetId(1),
                terminals: vec![Terminal::at((0, 0)), Terminal::at((2, 2))],
            },
        ];
        let mut engine = Engine::new(&space, &nets, options()).unwrap();
        while engine.step().is_some() {}
        assert_eq!(engine.table.slots.get((-1, 0)), None);
        let outcome = engine.outcome();
        assert_eq!(
            outcome.routes.unrouted().collect::<Vec<_>>(),
            [(NetId(0), UnroutedReason::NoPath)]
        );
        assert!(outcome.routes.route(NetId(1)).is_some());
    }

    // Errors.

    #[test]
    fn duplicate_nets_are_rejected() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let nets = [net(4, &[0, 1]), net(9, &[0, 1]), net(4, &[0, 1])];
        let error = route(&grid, &nets, options()).unwrap_err();
        assert_eq!(error, EngineError::DuplicateNet { net: NetId(4) });
        assert_eq!(error.to_string(), "net 4 is listed twice");
    }

    #[test]
    fn no_nets_is_a_clean_success() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let outcome = route_checked(&grid, &[], options());
        assert!(outcome.is_success());
        assert_eq!(outcome.stop, StopReason::Converged);
    }

    #[test]
    fn net_ids_need_not_be_dense_or_in_order() {
        let grid = Grid::new(3, 2, 1).unwrap();
        let nets = [net(900, &[3, 5]), net(7, &[0, 2])];
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
    }

    #[test]
    fn engine_debug_is_a_summary() {
        let grid = Grid::new(2, 1, 1).unwrap();
        let engine = Engine::new(&grid, &[], options()).unwrap();
        let text = format!("{engine:?}");
        assert!(text.contains("nets: 0"), "{text}");
    }

    // Spaces that are not dense.

    /// A lazy grid whose nodes and resources are coordinate pairs, whose resources do not map to
    /// any dense index, and whose search slots are interned.
    struct Lazy {
        size: i32,
    }

    impl RoutingSpace for Lazy {
        type Node = (i32, i32);
        type Resource = (i32, i32);
        type Slots = InternedSlots<(i32, i32)>;
        type ResourceSlots = InternedSlots<(i32, i32)>;

        fn new_slots(&self) -> Self::Slots {
            InternedSlots::new()
        }

        fn new_resource_slots(&self) -> Self::ResourceSlots {
            InternedSlots::new()
        }

        fn contains(&self, node: (i32, i32)) -> bool {
            (0..self.size).contains(&node.0) && (0..self.size).contains(&node.1)
        }

        fn expand(&self, _net: NetId, (x, y): (i32, i32), out: &mut Vec<Step<(i32, i32)>>) {
            for to in [(x + 1, y), (x, y + 1), (x - 1, y), (x, y - 1)] {
                if (0..self.size).contains(&to.0) && (0..self.size).contains(&to.1) {
                    out.push(Step {
                        to,
                        cost: Cost::ONE,
                    });
                }
            }
        }

        fn lower_bound(&self, _net: NetId, from: (i32, i32), to: (i32, i32)) -> Cost {
            Cost::from(from.0.abs_diff(to.0) + from.1.abs_diff(to.1))
        }

        fn resources(&self, _: NetId, _: (i32, i32), to: (i32, i32), out: &mut Vec<(i32, i32)>) {
            out.push(to);
        }

        fn capacity(&self, _resource: (i32, i32)) -> u32 {
            1
        }
    }

    #[test]
    fn works_on_generated_nodes_and_resources() {
        let space = Lazy { size: 6 };
        let nets = [
            Net {
                id: NetId(0),
                terminals: vec![Terminal::at((0, 1)), Terminal::at((5, 1))],
            },
            Net {
                id: NetId(1),
                terminals: vec![Terminal::at((1, 0)), Terminal::at((3, 3))],
            },
        ];
        let outcome = route_checked(&space, &nets, options());
        assert_legal(&space, &nets, &outcome);
    }

    // Determinism.

    /// A dense problem, for determinism and stress: every row's left end to a shuffled row's
    /// right end, on two layers.
    pub(super) fn permutation_problem(
        size: u32,
        layers: u32,
        seed: u64,
    ) -> (Grid, Vec<Net<NodeId>>) {
        let grid = Grid::new(size, size, layers).unwrap();
        let rows = permutation(size, seed);
        let nets = rows
            .iter()
            .enumerate()
            .map(|(y, &to)| {
                let from = grid.node_at(0, y as u32, 0).unwrap().0;
                let to = grid.node_at(size - 1, to, 0).unwrap().0;
                net(y as u32, &[from, to])
            })
            .collect();
        (grid, nets)
    }

    #[test]
    fn repeated_runs_are_identical() {
        let (grid, nets) = permutation_problem(12, 2, 3);
        let mut engine = Engine::new(&grid, &nets, options()).unwrap();
        while engine.step().is_some() {}
        let first = engine.outcome();
        let reports = engine.reports().to_vec();
        assert!(reports.len() > 1, "this should take some negotiation");
        for _ in 0..3 {
            let mut again = Engine::new(&grid, &nets, options()).unwrap();
            while again.step().is_some() {}
            assert_eq!(again.outcome(), first);
            assert_eq!(again.reports(), reports);
        }
    }

    #[test]
    fn shuffled_runs_repeat_for_a_seed_and_differ_between_seeds() {
        let (grid, nets) = permutation_problem(12, 2, 3);
        let seeded = |seed| {
            let shuffled = EngineOptions {
                shuffle_seed: Some(seed),
                ..options()
            };
            route_checked(&grid, &nets, shuffled)
        };
        let first = seeded(1);
        assert_eq!(seeded(1), first);
        assert!(first.is_success());
        let differs = (2..10).any(|seed| seeded(seed) != first);
        assert!(differs);
    }

    #[test]
    fn input_order_is_the_only_order_that_matters() {
        // Hash seeds differ between hash maps, which must not show. Each search map is new here.
        let (grid, nets) = permutation_problem(10, 2, 8);
        let expected = route_checked(&grid, &nets, options());
        let spaces: Vec<Grid> = (0..4).map(|_| grid.clone()).collect();
        for space in &spaces {
            assert_eq!(route_checked(space, &nets, options()), expected);
        }
    }

    /// A regression value and not a reference one: it came from this implementation, and it
    /// pins the combined behaviour of the search, the prices and the schedule to the bit. Costs
    /// only ever go through `f64` addition and multiplication in a fixed order, which IEEE 754
    /// defines exactly, so it must come out the same on Linux, macOS and Windows.
    ///
    /// A deliberate change to routing will change it. Update it then, and not otherwise.
    #[test]
    fn routes_match_their_regression_value() {
        let (grid, nets) = permutation_problem(16, 2, 42);
        let outcome = route_checked(&grid, &nets, options());
        assert_legal(&grid, &nets, &outcome);
        assert_eq!(hash_outcome(&outcome), 13615641185186696971);
    }

    // Stress. These print how they converged, which `--nocapture` shows.

    /// Runs a problem to the end, printing its progress, and requires a legal result.
    fn stress<S: RoutingSpace>(space: &S, nets: &[Net<S::Node>]) -> Outcome<S::Node, S::Resource> {
        let mut engine = Engine::new(space, nets, options()).unwrap();
        while let Some(report) = engine.step() {
            println!(
                "iteration {}: rerouted {}, expanded {}, overused {}, cost {}",
                report.iteration,
                report.nets_rerouted,
                report.nodes_expanded,
                report.total_overuse,
                report.total_cost
            );
        }
        let outcome = engine.outcome();
        assert_legal(space, nets, &outcome);
        outcome
    }

    /// As dense as this kind of problem gets: every cell of both side columns is a terminal, and
    /// with two layers almost all of the crossings have to be negotiated.
    #[test]
    fn dense_grid_stress_converges() {
        let (grid, nets) = permutation_problem(32, 2, 7);
        let outcome = stress(&grid, &nets);
        assert!(outcome.routes.stats.iterations > 3);
    }

    /// Larger, and with nets of up to four terminals, at a density that takes negotiating.
    #[test]
    fn dense_multi_terminal_grid_stress_converges() {
        let grid = Grid::new(48, 48, 2).unwrap();
        let mut rng = crate::rng::Rng::new(11);
        let mut cells = permutation(48 * 48, 12);
        let nets: Vec<_> = (0..40)
            .map(|id| {
                let count = 2 + rng.below(3) as usize;
                let at = cells.split_off(cells.len() - count);
                net(id, &at)
            })
            .collect();
        let outcome = stress(&grid, &nets);
        assert!(outcome.routes.stats.iterations > 1);
    }

    #[test]
    fn large_permutation_stress_converges() {
        let (grid, nets) = permutation_problem(48, 2, 1);
        let outcome = stress(&grid, &nets);
        assert!(outcome.routes.stats.iterations > 3);
    }

    /// An island-style FPGA fabric: a square of tiles, each with one pin and a number of tracks
    /// in each direction. The width of the channels is the one that matters here.
    struct Fabric {
        size: u32,
        tracks: u32,
    }

    impl Fabric {
        fn per_tile(&self) -> u32 {
            2 * self.tracks + 1
        }

        fn pin(&self, x: u32, y: u32) -> u32 {
            (x + self.size * y) * self.per_tile()
        }

        fn wire(&self, x: u32, y: u32, vertical: bool, track: u32) -> u32 {
            self.pin(x, y) + 1 + track * 2 + u32::from(vertical)
        }

        fn build(&self) -> impl RoutingSpace<Node = NodeId, Resource = NodeId> + use<> {
            let mut builder = GraphBuilder::new(self.size * self.size * self.per_tile());
            let mut both_ways = |a: u32, b: u32| {
                builder.add_edge(NodeId(a), NodeId(b), Cost::ONE).unwrap();
                builder.add_edge(NodeId(b), NodeId(a), Cost::ONE).unwrap();
            };
            for y in 0..self.size {
                for x in 0..self.size {
                    for track in 0..self.tracks {
                        let horizontal = self.wire(x, y, false, track);
                        let vertical = self.wire(x, y, true, track);
                        both_ways(self.pin(x, y), horizontal);
                        both_ways(self.pin(x, y), vertical);
                        both_ways(horizontal, vertical);
                        both_ways(horizontal, self.wire(x, y, true, (track + 1) % self.tracks));
                        if x + 1 < self.size {
                            both_ways(horizontal, self.wire(x + 1, y, false, track));
                        }
                        if y + 1 < self.size {
                            both_ways(vertical, self.wire(x, y + 1, true, track));
                        }
                    }
                }
            }
            // Each step costs 1 and moves at most one tile.
            let (size, per_tile) = (self.size, self.per_tile());
            builder
                .build()
                .with_lower_bound(move |from: NodeId, to: NodeId| {
                    let (from, to) = (from.0 / per_tile, to.0 / per_tile);
                    Cost::from(
                        (from % size).abs_diff(to % size) + (from / size).abs_diff(to / size),
                    )
                })
        }

        /// Nets of two to four terminals, on pins that no two nets share.
        fn nets(&self, count: u32, seed: u64) -> Vec<Net<NodeId>> {
            let mut rng = crate::rng::Rng::new(seed);
            let mut tiles: Vec<u32> = (0..self.size * self.size).collect();
            rng.shuffle(&mut tiles);
            let mut tiles = tiles.into_iter();
            (0..count)
                .map(|id| {
                    let terminals = 2 + rng.below(3);
                    let pins: Vec<u32> = (0..terminals)
                        .filter_map(|_| tiles.next())
                        .map(|tile| self.pin(tile % self.size, tile / self.size))
                        .collect();
                    net(id, &pins)
                })
                .collect()
        }
    }

    /// Five tracks is the narrowest width that this problem routes on.
    #[test]
    fn fabric_at_minimum_channel_width_converges() {
        let fabric = Fabric {
            size: 16,
            tracks: 5,
        };
        let (space, nets) = (fabric.build(), fabric.nets(80, 2));
        let outcome = stress(&space, &nets);
        assert!(outcome.routes.stats.iterations >= 2);
    }

    /// Far too few tracks, and the result has to say that it did not work.
    #[test]
    fn fabric_below_minimum_channel_width_reports_failure() {
        let fabric = Fabric {
            size: 12,
            tracks: 1,
        };
        let (space, nets) = (fabric.build(), fabric.nets(40, 2));
        let limited = EngineOptions {
            max_iterations: 15,
            ..options()
        };
        let outcome = route_checked(&space, &nets, limited);
        assert!(!outcome.is_success());
        assert_eq!(outcome.stop, StopReason::IterationLimit);
        assert!(!outcome.overused.is_empty());
    }

    // Properties.

    /// Random terminals over `0..nodes`, as `(node, access cost)`, for up to `nets` nets.
    fn any_nets(nodes: u32, nets: usize) -> impl Strategy<Value = Vec<Net<NodeId>>> {
        let point = (0..nodes, 0..4u32);
        let terminal = prop::collection::vec(point, 1..3).prop_map(|points| terminal(&points));
        prop::collection::vec(prop::collection::vec(terminal, 0..5), 0..nets).prop_map(|list| {
            list.into_iter()
                .enumerate()
                .map(|(id, terminals)| Net {
                    id: NetId(id as u32 * 3 + 1),
                    terminals,
                })
                .collect()
        })
    }

    /// A small grid with random blocks and capacities.
    pub(super) fn any_grid_problem() -> impl Strategy<Value = (Grid, Vec<Net<NodeId>>)> {
        (2u32..7, 2u32..7, 1u32..3).prop_flat_map(|(width, height, layers)| {
            let cells = (width * height * layers) as usize;
            (
                Just((width, height, layers)),
                prop::collection::vec((prop::bool::weighted(0.1), 0u32..4), cells),
                any_nets(width * height * layers, 8),
            )
                .prop_map(|((width, height, layers), cells, nets)| {
                    let mut grid = Grid::new(width, height, layers).unwrap();
                    for (id, &(blocked, capacity)) in cells.iter().enumerate() {
                        let (x, y, layer) = grid.position(NodeId(id as u32)).unwrap();
                        grid.set_blocked(x, y, layer, blocked).unwrap();
                        // Mostly the default of 1, with some zero and some larger.
                        let capacity = [1, 1, 0, 2][capacity as usize];
                        grid.set_capacity(x, y, layer, capacity).unwrap();
                    }
                    (grid, nets)
                })
        })
    }

    /// A small directed graph with random costs, including free steps, and random capacities.
    pub(super) fn any_graph_problem() -> impl Strategy<Value = (Graph, Vec<Net<NodeId>>)> {
        (2u32..20).prop_flat_map(|nodes| {
            (
                prop::collection::vec((0..nodes, 0..nodes, 0..4u32), 0..60),
                prop::collection::vec(0u32..4, nodes as usize),
                any_nets(nodes, 8),
            )
                .prop_map(move |(edges, capacities, nets)| {
                    let mut builder = GraphBuilder::new(nodes);
                    for (from, to, cost) in edges {
                        builder
                            .add_edge(NodeId(from), NodeId(to), Cost::from(cost))
                            .unwrap();
                    }
                    for (node, &capacity) in capacities.iter().enumerate() {
                        builder
                            .set_capacity(NodeId(node as u32), [1, 1, 0, 2][capacity as usize])
                            .unwrap();
                    }
                    (builder.build(), nets)
                })
        })
    }

    /// The checks that any problem has to pass, on any space.
    fn check_problem<S: RoutingSpace>(
        space: &S,
        nets: &[Net<S::Node>],
    ) -> Result<(), TestCaseError> {
        let limited = EngineOptions {
            max_iterations: 12,
            ..options()
        };
        let outcome = route(space, nets, limited).unwrap();

        // Whatever it claims, it must be the truth.
        let honest = check_honest(space, nets, &outcome);
        prop_assert!(honest.is_ok(), "{:?}", honest);
        if outcome.is_success() {
            let legal = check_legal(space, nets, &outcome.routes);
            prop_assert!(legal.is_ok(), "{:?}", legal);
        }

        // A net is routed exactly when a path exists, as no budget was set.
        for net in nets {
            let can = routable(space, net);
            let result = outcome.routes.outcome(net.id);
            match result {
                Some(NetOutcome::Routed(_)) => {
                    prop_assert!(can, "{:?} routed without a path", net.id)
                }
                Some(NetOutcome::Unrouted(UnroutedReason::NoPath)) => {
                    prop_assert!(!can, "{:?} reported as no path, but has one", net.id);
                }
                other => prop_assert!(false, "{:?} ended up as {:?}", net.id, other),
            }
        }

        // Stepping and running agree, as do repeated runs.
        let mut engine = Engine::new(space, nets, limited).unwrap();
        while engine.step().is_some() {}
        prop_assert_eq!(engine.outcome(), outcome);
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn grid_problems_get_honest_results((grid, nets) in any_grid_problem()) {
            check_problem(&grid, &nets)?;
        }

        #[test]
        fn graph_problems_get_honest_results((graph, nets) in any_graph_problem()) {
            check_problem(&graph, &nets)?;
        }

        #[test]
        fn partial_budgets_give_honest_results(
            (grid, nets) in any_grid_problem(),
            budget in 0u64..200,
        ) {
            let limited = EngineOptions {
                max_iterations: 8,
                max_expansions: budget,
                ..options()
            };
            let outcome = route(&grid, &nets, limited).unwrap();
            let honest = check_honest(&grid, &nets, &outcome);
            prop_assert!(honest.is_ok(), "{:?}", honest);
            prop_assert!(outcome.routes.stats.nodes_expanded <= budget);
            for (id, reason) in outcome.routes.unrouted() {
                let net = nets.iter().find(|net| net.id == id).unwrap();
                if reason == UnroutedReason::NoPath {
                    prop_assert!(!routable(&grid, net));
                } else {
                    prop_assert_eq!(reason, UnroutedReason::BudgetExhausted);
                }
            }
        }
    }
}
