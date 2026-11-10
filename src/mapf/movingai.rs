//! Reading the MovingAI benchmarks for multi-agent path finding: `.map` grids and `.scen`
//! scenarios.
//!
//! The benchmarks (Stern et al., 2019) are at <https://movingai.com/benchmarks/mapf.html>,
//! under the Open Data Commons Attribution License. A map is a header and a grid of
//! characters:
//!
//! ```text
//! type octile
//! height 4
//! width 4
//! map
//! ....
//! .@@.
//! ....
//! ....
//! ```
//!
//! `.`, `G` and `S` are open, and anything else is blocked, as the MAPF benchmarks treat
//! them. A scenario lists agents, one per line, after a `version 1` line, as tab-separated
//! fields: a bucket, the map's file name, its width and height, the start's `x` and `y`, the
//! goal's `x` and `y`, and the length of an optimal path for a lone agent that may move
//! diagonally, which multi-agent path finding doesn't use. `y` counts rows from the top.
//! An instance with `n` agents is the first `n` lines.

use core::fmt;
use std::error::Error;

use super::graph::{Agent, Graph, GraphError};

/// A map: a grid of open and blocked cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Map {
    /// Columns.
    pub width: u32,
    /// Rows.
    pub height: u32,
    /// Whether each cell is open, in row-major order from the top.
    pub open: Vec<bool>,
}

impl Map {
    /// Builds the 4-connected graph of the open cells.
    ///
    /// # Errors
    ///
    /// Returns an error only if the grid is too large for a `u32` to count its cells.
    pub fn graph(&self) -> Result<Graph, GraphError> {
        Graph::from_grid(self.width, self.height, &self.open)
    }
}

/// One line of a scenario.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    /// The bucket, which groups tasks of similar length.
    pub bucket: u32,
    /// The start cell, as `(x, y)`.
    pub start: (u32, u32),
    /// The goal cell, as `(x, y)`.
    pub goal: (u32, u32),
}

/// What went wrong reading a map or a scenario.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// A line of the header is missing or wrong.
    Header {
        /// The line, from 1.
        line: usize,
    },
    /// A row of the grid is missing or has the wrong width.
    Row {
        /// The line, from 1.
        line: usize,
    },
    /// A task's line doesn't have its nine fields, or a number in it is wrong.
    Task {
        /// The line, from 1.
        line: usize,
    },
    /// A task's cell is blocked or outside the map.
    BlockedCell {
        /// The task, from 0.
        task: usize,
    },
    /// The map is too large.
    TooLarge,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Header { line } => write!(f, "bad header at line {line}"),
            ParseError::Row { line } => write!(f, "bad row at line {line}"),
            ParseError::Task { line } => write!(f, "bad task at line {line}"),
            ParseError::BlockedCell { task } => {
                write!(f, "task {task} starts or ends on a blocked cell")
            }
            ParseError::TooLarge => f.write_str("the map is too large"),
        }
    }
}

impl Error for ParseError {}

/// Reads a `.map` file's text.
///
/// # Errors
///
/// Returns an error for a bad header or a row of the wrong width or count.
pub fn parse_map(text: &str) -> Result<Map, ParseError> {
    let mut lines = text.lines().map(str::trim_end).enumerate();
    let mut width = None;
    let mut height = None;
    loop {
        let Some((index, line)) = lines.next() else {
            return Err(ParseError::Header {
                line: index_after(text),
            });
        };
        let mut words = line.split_whitespace();
        let bad = ParseError::Header { line: index + 1 };
        match words.next() {
            Some("type") => {}
            Some("height") => height = Some(number(words.next()).ok_or(bad)?),
            Some("width") => width = Some(number(words.next()).ok_or(bad)?),
            Some("map") => break,
            _ => return Err(bad),
        }
    }
    let (Some(width), Some(height)) = (width, height) else {
        return Err(ParseError::Header { line: 1 });
    };
    let cells = (width as usize)
        .checked_mul(height as usize)
        .filter(|&cells| u32::try_from(cells).is_ok())
        .ok_or(ParseError::TooLarge)?;
    let mut open = Vec::with_capacity(cells);
    for row in 0..height as usize {
        let (index, line) = lines.next().ok_or(ParseError::Row {
            line: index_after(text) + row,
        })?;
        if line.len() != width as usize {
            return Err(ParseError::Row { line: index + 1 });
        }
        open.extend(line.bytes().map(|cell| matches!(cell, b'.' | b'G' | b'S')));
    }
    Ok(Map {
        width,
        height,
        open,
    })
}

/// The number of the line after the last of `text`, from 1.
fn index_after(text: &str) -> usize {
    text.lines().count() + 1
}

fn number(word: Option<&str>) -> Option<u32> {
    word?.parse().ok()
}

/// Reads a `.scen` file's text.
///
/// # Errors
///
/// Returns an error for a line without nine fields or with a number that doesn't read.
pub fn parse_scenario(text: &str) -> Result<Vec<Task>, ParseError> {
    let mut tasks = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || (index == 0 && line.starts_with("version")) {
            continue;
        }
        let bad = ParseError::Task { line: index + 1 };
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 9 {
            return Err(bad);
        }
        let field = |at: usize| number(Some(fields[at])).ok_or(bad.clone());
        tasks.push(Task {
            bucket: field(0)?,
            start: (field(4)?, field(5)?),
            goal: (field(6)?, field(7)?),
        });
    }
    Ok(tasks)
}

/// Returns the agents of the first `count` tasks on `graph`, a graph built from the map the
/// tasks are for.
///
/// # Errors
///
/// Returns an error for a task with a cell that isn't a vertex of the graph.
pub fn agents(graph: &Graph, tasks: &[Task], count: usize) -> Result<Vec<Agent>, ParseError> {
    tasks
        .iter()
        .take(count)
        .enumerate()
        .map(|(index, task)| {
            let vertex = |(x, y)| {
                graph
                    .vertex_at(x, y)
                    .ok_or(ParseError::BlockedCell { task: index })
            };
            Ok(Agent {
                start: vertex(task.start)?,
                goal: vertex(task.goal)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "type octile\nheight 3\nwidth 4\nmap\n....\n.@T.\nG..S\n";

    #[test]
    fn maps_read_open_and_blocked_cells() {
        let map = parse_map(MAP).unwrap();
        assert_eq!((map.width, map.height), (4, 3));
        assert_eq!(
            map.open,
            [
                true, true, true, true, true, false, false, true, true, true, true, true
            ]
        );
        assert_eq!(map.graph().unwrap().vertex_count(), 10);
        assert_eq!(
            parse_map("type octile\nheight 2\nwidth 2\nmap\n..\n"),
            Err(ParseError::Row { line: 7 })
        );
        assert_eq!(
            parse_map("type octile\nheight 1\nwidth 2\nmap\n...\n"),
            Err(ParseError::Row { line: 5 })
        );
        assert_eq!(
            parse_map("type octile\nheight x\n"),
            Err(ParseError::Header { line: 2 })
        );
        assert_eq!(
            parse_map("type octile\nmap\n"),
            Err(ParseError::Header { line: 1 })
        );
    }

    #[test]
    fn scenarios_read_tasks_onto_the_graph() {
        let text = "version 1\n0\tm.map\t4\t3\t0\t0\t3\t2\t5.0\n\
                    1\tm.map\t4\t3\t3\t0\t0\t2\t3.0\n\
                    1\tm.map\t4\t3\t1\t1\t0\t2\t3.0\n";
        let tasks = parse_scenario(text).unwrap();
        assert_eq!(
            tasks[0],
            Task {
                bucket: 0,
                start: (0, 0),
                goal: (3, 2)
            }
        );
        let graph = parse_map(MAP).unwrap().graph().unwrap();
        let agents = agents(&graph, &tasks, 2).unwrap();
        assert_eq!(
            agents,
            [Agent { start: 0, goal: 9 }, Agent { start: 3, goal: 6 }]
        );
        assert_eq!(
            super::agents(&graph, &tasks, 3),
            Err(ParseError::BlockedCell { task: 2 })
        );
        assert_eq!(
            parse_scenario("version 1\n0\tm.map\t4\n"),
            Err(ParseError::Task { line: 2 })
        );
        assert_eq!(
            parse_scenario("0\tm.map\t4\t3\tx\t0\t3\t2\t5.0\n"),
            Err(ParseError::Task { line: 1 })
        );
    }
}
