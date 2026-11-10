use super::*;
use crate::rng::Rng;

/// A random instance on a `size` x `size` grid with about a fifth of its cells blocked.
fn random_instance(seed: u64, size: u32, agents: usize) -> (Graph, Vec<Agent>) {
    let mut rng = Rng::new(seed);
    let open: Vec<bool> = (0..size * size).map(|_| rng.below(5) != 0).collect();
    let graph = Graph::from_grid(size, size, &open).unwrap();
    // Pairs that don't connect are drawn again.
    let n = graph.vertex_count();
    let mut used_start = vec![false; n as usize];
    let mut used_goal = vec![false; n as usize];
    let mut list = Vec::new();
    let mut tries = 0;
    while list.len() < agents && tries < 10_000 {
        tries += 1;
        let start = rng.below(u64::from(n)) as u32;
        let goal = rng.below(u64::from(n)) as u32;
        if used_start[start as usize] || used_goal[goal as usize] {
            continue;
        }
        if graph.distances_to(goal)[start as usize] == u32::MAX {
            continue;
        }
        used_start[start as usize] = true;
        used_goal[goal as usize] = true;
        list.push(Agent { start, goal });
    }
    (graph, list)
}

#[test]
fn agents_pass_each_other_in_a_corridor_with_a_siding() {
    // 0 - 1 - 2 - 3 - 4
    //         |
    //         5
    let graph = Graph::from_edges(6, &[(0, 1), (1, 2), (2, 3), (3, 4), (2, 5)]).unwrap();
    let agents = vec![Agent { start: 0, goal: 4 }, Agent { start: 4, goal: 0 }];
    let instance = Instance::new(&graph, agents.clone()).unwrap();
    let solution = solve(&instance, &SolveOptions::default());
    let plans = solution.plans.as_ref().unwrap();
    let summary = check::check(&graph, &agents, plans).unwrap();
    // One agent steps into the siding and out again, and the other waits for it: 5 + 6, as
    // any other way round costs as much.
    assert_eq!(summary.sum_of_costs, 11);
    assert_eq!(solution.sum_of_costs(), Some(11));
    assert_eq!(solution.makespan(), Some(6));
}

#[test]
fn an_agent_on_its_goal_moves_aside_and_comes_back() {
    // Agent 1 starts on its goal in the middle of a corridor with a siding, and agent 0 has
    // to get past it.
    let graph = Graph::from_edges(5, &[(0, 1), (1, 2), (2, 3), (1, 4)]).unwrap();
    let agents = vec![Agent { start: 0, goal: 3 }, Agent { start: 1, goal: 1 }];
    let instance = Instance::new(&graph, agents.clone()).unwrap();
    let solution = solve(&instance, &SolveOptions::default());
    let plans = solution.plans.as_ref().unwrap();
    assert!(check::check(&graph, &agents, plans).is_ok());
    assert_eq!(solution.sum_of_costs(), Some(3 + 2));
}

#[test]
fn a_corridor_without_a_siding_has_no_solution() {
    let graph = Graph::from_edges(3, &[(0, 1), (1, 2)]).unwrap();
    let agents = vec![Agent { start: 0, goal: 2 }, Agent { start: 2, goal: 0 }];
    let instance = Instance::new(&graph, agents).unwrap();
    let mut options = SolveOptions::default();
    options.engine.max_iterations = 20;
    options.rounds = 2;
    let solution = solve(&instance, &options);
    assert_eq!(solution.plans, None);
    assert_eq!(solution.rounds.len(), 2);
    assert!(solution.rounds[1].horizon > solution.rounds[0].horizon);
    assert!(solution.rounds.iter().all(|round| round.conflicts > 0));
    assert_eq!(solution.sum_of_costs(), None);
}

#[test]
fn random_grids_solve_legally_and_the_same_on_any_thread_count() {
    for seed in 0..6 {
        let (graph, agents) = random_instance(seed, 12, 20);
        let instance = Instance::new(&graph, agents.clone()).unwrap();
        let mut options = SolveOptions::default();
        options.engine.batch_size = 4;
        let one = solve(&instance, &options);
        options.threads = 3;
        let three = solve(&instance, &options);
        assert_eq!(one, three);
        let plans = one.plans.as_ref().expect("a sparse grid solves");
        let summary = check::check(&graph, &agents, plans).unwrap();
        assert_eq!(Some(summary.sum_of_costs), one.sum_of_costs());
        assert_eq!(Some(summary.makespan), one.makespan());
        assert!(summary.sum_of_costs >= instance.sum_of_shortest());
        assert!(summary.makespan >= instance.longest_shortest());
    }
}

#[test]
fn an_expansion_budget_stops_the_solve() {
    let (graph, agents) = random_instance(3, 12, 30);
    let instance = Instance::new(&graph, agents).unwrap();
    let mut options = SolveOptions::default();
    options.engine.max_expansions = 50;
    let solution = solve(&instance, &options);
    assert_eq!(solution.plans, None);
    assert_eq!(solution.rounds.len(), 1);
    assert_eq!(solution.rounds[0].stop, StopReason::BudgetExhausted);
}
