//! Solves MovingAI MAPF instances and prints one tab-separated line per instance.
//!
//! ```text
//! cargo run --release --features mapf --example mapf_bench -- \
//!     MAP SCEN AGENTS[,AGENTS...] [--time SECONDS] [--slack N] [--rounds N]
//!     [--first-present X] [--initial-present X] [--growth X] [--history X]
//!     [--iterations N] [--batch N] [--threads N] [--incremental] [--improve PASSES]
//!     [--method sipp|time] [--first 0|1]
//! ```
//!
//! The columns are: scenario, agents, solved, sum of costs, the sum of the agents' shortest
//! paths, makespan, milliseconds, iterations, expansions, rounds, the last round's horizon, the
//! sum of costs before shortening, the sum of costs of the first solution (LaCAM's), the
//! milliseconds to it, and 1 if the plans are that first solution's, shortened, because
//! negotiation didn't settle. Both times include working out the distances to every goal.
//! Every solution is checked by `guide::mapf::check` before it is printed, and a solution that
//! fails the check stops the program.
//!
//! The time limit sets the engine's cancel flag from a timer, so a run that hits it isn't
//! reproducible. Without one, a run is.

use std::cell::Cell;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use guide::control::{Progress, RunControl, Stage};
use guide::cost::Cost;
use guide::mapf::{Instance, Method, SolveOptions, check, movingai, solve_with};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("mapf_bench: {message}");
            ExitCode::FAILURE
        }
    }
}

fn cost(text: &str) -> Result<Cost, String> {
    text.parse::<f64>()
        .ok()
        .and_then(Cost::new)
        .ok_or_else(|| format!("bad cost {text}"))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [map_path, scen_path, counts, rest @ ..] = args.as_slice() else {
        return Err("usage: mapf_bench MAP SCEN AGENTS[,AGENTS...] [options]".into());
    };
    let mut options = SolveOptions::default();
    let mut time_limit = None;
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        if flag == "--incremental" {
            options.engine.incremental_reroute = true;
            continue;
        }
        let value = rest.next().ok_or(format!("{flag} needs a value"))?;
        let number = || {
            value
                .parse::<u32>()
                .map_err(|_| format!("bad number {value}"))
        };
        match flag.as_str() {
            "--time" => {
                let seconds: f64 = value.parse().map_err(|_| format!("bad time {value}"))?;
                time_limit = Some(Duration::from_secs_f64(seconds));
            }
            "--slack" => options.slack = number()?,
            "--improve" => options.improve_passes = number()?,
            "--rounds" => options.rounds = number()?,
            "--iterations" => options.engine.max_iterations = number()?,
            "--batch" => options.engine.batch_size = number()?,
            "--threads" => options.threads = number()? as usize,
            "--first-present" => options.engine.first_present = cost(value)?,
            "--initial-present" => options.engine.initial_present = cost(value)?,
            "--growth" => options.engine.present_growth = cost(value)?,
            "--history" => options.engine.history_factor = cost(value)?,
            "--first" => options.first = number()? != 0,
            "--method" => {
                options.method = match value.as_str() {
                    "time" => Method::TimeExpanded,
                    "sipp" => Method::SafeIntervals,
                    _ => return Err(format!("unknown method {value}")),
                }
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    let read = |path: &str| std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    let map = movingai::parse_map(&read(map_path)?).map_err(|e| format!("{map_path}: {e}"))?;
    let tasks =
        movingai::parse_scenario(&read(scen_path)?).map_err(|e| format!("{scen_path}: {e}"))?;
    let graph = map.graph().map_err(|e| e.to_string())?;
    let scen_name = std::path::Path::new(scen_path)
        .file_name()
        .map_or(scen_path.as_str(), |name| {
            name.to_str().unwrap_or(scen_path)
        });

    for count in counts.split(',') {
        let count: usize = count.parse().map_err(|_| format!("bad count {count}"))?;
        if count > tasks.len() {
            continue;
        }
        let agents = movingai::agents(&graph, &tasks, count).map_err(|e| e.to_string())?;

        // Times count from here, with the distances to every goal that the instance works out.
        let started = Instant::now();
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let timer = time_limit.map(|limit| {
            let (cancel, done) = (Arc::clone(&cancel), Arc::clone(&done));
            std::thread::spawn(move || {
                let (lock, signal) = &*done;
                let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                let (guard, _) = signal
                    .wait_timeout_while(guard, limit, |finished| !*finished)
                    .unwrap_or_else(|e| e.into_inner());
                if !*guard {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
        });
        let instance = Instance::new(&graph, agents.clone()).map_err(|e| e.to_string())?;
        let setup = started.elapsed();
        let mut run_options = options;
        run_options.engine.cancel = Some(&cancel);
        // The first solution's time: solve tells of negotiation's start once it has one.
        let solving = Instant::now();
        let first_at = Cell::new(None);
        let heard = |progress: &Progress| {
            if progress.stage == Stage::Route && first_at.get().is_none() {
                first_at.set(Some(solving.elapsed()));
            }
        };
        let mut control = RunControl::default();
        control.progress = Some(&heard);
        let solution = solve_with(&instance, &run_options, &control);
        let first_ms = match first_at.get() {
            Some(first) if run_options.first => (setup + first).as_secs_f64() * 1000.0,
            _ => 0.0,
        };
        let elapsed = setup + solving.elapsed();
        {
            let (lock, signal) = &*done;
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
            signal.notify_all();
        }
        if let Some(timer) = timer {
            let _ = timer.join();
        }

        let (solved, soc, makespan) = match &solution.plans {
            Some(plans) => {
                let summary = check::check(&graph, &agents, plans).map_err(|faults| {
                    format!("{scen_name} with {count} agents: illegal plans: {faults:?}")
                })?;
                (
                    1,
                    summary.sum_of_costs.to_string(),
                    summary.makespan.to_string(),
                )
            }
            None => (0, "-".into(), "-".into()),
        };
        let last = solution.rounds.last();
        println!(
            "{scen_name}\t{count}\t{solved}\t{soc}\t{}\t{makespan}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{first_ms:.1}\t{}",
            instance.sum_of_shortest(),
            elapsed.as_millis(),
            solution.iterations(),
            solution.nodes_expanded(),
            solution.rounds.len(),
            last.map_or(0, |round| round.horizon),
            solution
                .negotiated_sum_of_costs
                .map_or("-".into(), |soc| soc.to_string()),
            solution
                .first_sum_of_costs
                .map_or("-".into(), |soc| soc.to_string()),
            u8::from(solution.fell_back),
        );
    }
    Ok(())
}
