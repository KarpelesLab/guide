//! `guide mapf`: multi-agent path finding on MovingAI maps.

use std::fmt::Write as _;

use guide::cost::Cost;
use guide::mapf::{Instance, SolveOptions, check, movingai};

use crate::Outcome;
use crate::args::{self, Command, Failure, Opt};
use crate::report::{Deadline, Map, Ui, read_text, write_output};

fn solve_command() -> Command {
    Command {
        name: "mapf solve",
        about: "Plans paths for the agents of a MovingAI scenario on its map, so that no two are \
                ever on one cell or swap cells at once, for the least sum of costs.",
        operands: "MAP.map SCENARIO.scen",
        count: (2, 2),
        options: vec![
            Opt::value(
                "agents",
                "N",
                "Take the scenario's first N agents [default: all of them]",
            ),
            args::output(
                "Where to write the plans, as LaCAM does: the cell of every agent at each step \
                 (`-` for standard output)",
            ),
            Opt::value(
                "improve",
                "N",
                "Passes over the agents after a solution, to shorten their paths [default: 10]",
            ),
            Opt::value(
                "rounds",
                "N",
                "Rounds of negotiation, each with a longer horizon [default: 4]",
            ),
            Opt::value(
                "max-iterations",
                "N",
                "Iterations of negotiation per round at most [default: 100]",
            ),
            Opt::value(
                "history",
                "F",
                "How much each conflict adds to the price of its cell [default: 0.5]",
            ),
            Opt::value(
                "batch",
                "N",
                "Agents planned at once, which changes results; threads help only above 1 \
                 [default: 1]",
            ),
            Opt::value(
                "seed",
                "N",
                "Shuffle the agents between iterations with this seed [default: no shuffle]",
            ),
            args::THREADS,
            args::TIME_LIMIT,
            args::JSON,
            args::QUIET,
            args::VERBOSE,
        ],
        details: "Example:\n  guide mapf solve random-32-32-20.map random-32-32-20-random-1.scen \
                  --agents 100 -o plans.txt\n\n\
                  Every solution is checked by an independent checker before it is written.",
    }
}

/// `guide mapf solve`.
pub fn solve(argv: &[String]) -> Outcome {
    let m = solve_command().parse(argv)?;
    let ui = Ui::new(&m);
    let (map_path, scen_path) = (&m.operands[0], &m.operands[1]);
    let mut options = SolveOptions::default();
    if let Some(n) = m.number("improve")? {
        options.improve_passes = n;
    }
    if let Some(n) = m.number("rounds")? {
        options.rounds = n;
    }
    if let Some(n) = m.number("max-iterations")? {
        options.engine.max_iterations = n;
    }
    if let Some(n) = m.number("batch")? {
        options.engine.batch_size = n;
    }
    if let Some(f) = m.factor("history")? {
        options.engine.history_factor =
            Cost::new(f).ok_or_else(|| Failure::Usage(format!("--history {f} is too large")))?;
    }
    options.engine.shuffle_seed = m.number("seed")?;
    options.threads = m.threads()?;
    let limit = m.seconds("time-limit")?;

    ui.progress(format!("reading {map_path} and {scen_path}"));
    let map = movingai::parse_map(&read_text(map_path)?).map_err(|e| Failure::file(map_path, e))?;
    let tasks = movingai::parse_scenario(&read_text(scen_path)?)
        .map_err(|e| Failure::file(scen_path, e))?;
    let graph = map.graph().map_err(|e| Failure::file(map_path, e))?;
    let count = match m.number::<usize>("agents")? {
        Some(n) if n > tasks.len() => {
            return Err(Failure::Usage(format!(
                "--agents {n}: the scenario has {} agents",
                tasks.len()
            )));
        }
        Some(n) => n,
        None => tasks.len(),
    };
    let agents =
        movingai::agents(&graph, &tasks, count).map_err(|e| Failure::file(scen_path, e))?;
    let instance =
        Instance::new(&graph, agents.clone()).map_err(|e| Failure::file(scen_path, e))?;

    ui.progress(format!("solving for {count} agents"));
    let deadline = Deadline::start(limit);
    options.engine.cancel = deadline.flag();
    let solution = guide::mapf::solve(&instance, &options);
    for (i, round) in solution.rounds.iter().enumerate() {
        ui.detail(format!(
            "round {i}: horizon {} iterations {} conflicts {} stop {:?}",
            round.horizon, round.iterations, round.conflicts, round.stop
        ));
    }

    let mut report = Map::default();
    report
        .set("solved", solution.plans.is_some())
        .set("agents", count)
        .set("sum_of_shortest_paths", instance.sum_of_shortest())
        .set("time_limit_hit", deadline.expired());
    let mut written = None;
    let solved = match &solution.plans {
        Some(plans) => {
            let summary = check::check(&graph, &agents, plans).map_err(|faults| {
                Failure::Input(format!(
                    "internal error: the solution is illegal: {faults:?}"
                ))
            })?;
            report
                .set("sum_of_costs", summary.sum_of_costs)
                .set("makespan", summary.makespan);
            if let Some(path) = m.get("output") {
                let cell = |v: u32| graph.cell(v).unwrap_or((u32::MAX, u32::MAX));
                let mut text = String::new();
                let _ = writeln!(text, "agents={count}");
                let _ = writeln!(text, "map_file={map_path}");
                let _ = writeln!(text, "solver=guide");
                let _ = writeln!(text, "solved=1");
                let _ = writeln!(text, "soc={}", summary.sum_of_costs);
                let _ = writeln!(text, "makespan={}", summary.makespan);
                let list = |text: &mut String, key: &str, at: &dyn Fn(usize) -> u32| {
                    text.push_str(key);
                    text.push('=');
                    for i in 0..agents.len() {
                        let (x, y) = cell(at(i));
                        let _ = write!(text, "({x},{y}),");
                    }
                    text.push('\n');
                };
                list(&mut text, "starts", &|i| agents[i].start);
                list(&mut text, "goals", &|i| agents[i].goal);
                text.push_str("solution=\n");
                let steps = plans.iter().map(Vec::len).max().unwrap_or(0);
                for t in 0..steps {
                    let _ = write!(text, "{t}:");
                    for plan in plans {
                        // An agent that arrived stays on its goal.
                        let v = plan.get(t).or(plan.last()).copied().unwrap_or(0);
                        let (x, y) = cell(v);
                        let _ = write!(text, "({x},{y}),");
                    }
                    text.push('\n');
                }
                write_output(path, text.as_bytes())?;
                written = Some(path);
            }
            true
        }
        None => false,
    };
    report
        .set("iterations", solution.iterations())
        .set("expansions", solution.nodes_expanded())
        .set("rounds", solution.rounds.len())
        .set("written", written);
    ui.finish("mapf solve", report)?;
    Ok(solved)
}
