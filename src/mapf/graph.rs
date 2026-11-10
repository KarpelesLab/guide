//! The graphs agents move on, and the agents themselves.

use core::fmt;
use std::collections::VecDeque;
use std::error::Error;

/// An undirected graph of the places agents can be, as a compact adjacency list.
///
/// Vertices are numbered from 0. A graph built from a grid numbers its passable cells in
/// row-major order and keeps their coordinates, so that a plan can be read back as cells.
/// Each vertex lists its neighbours in a fixed order, which is the order a search tries them
/// in, and so the order ties are broken by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    /// Where each vertex's neighbours start in `neighbours`, with one entry more at the end.
    offsets: Vec<u32>,
    neighbours: Vec<u32>,
    /// The cell of each vertex, for a graph built from a grid.
    cells: Vec<(u32, u32)>,
    /// The vertex of each cell, or [`NO_VERTEX`], for a graph built from a grid.
    vertex_of_cell: Vec<u32>,
    width: u32,
    height: u32,
}

/// Marks a blocked cell in a grid's index of vertices.
const NO_VERTEX: u32 = u32::MAX;

/// Why a graph could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GraphError {
    /// An edge names a vertex at or beyond the number of vertices.
    VertexOutOfRange {
        /// The vertex.
        vertex: u32,
    },
    /// An edge joins a vertex to itself. Waiting is always allowed and needs no edge.
    SelfLoop {
        /// The vertex.
        vertex: u32,
    },
    /// The grid's cells are not `width * height` in number.
    WrongCellCount {
        /// How many there should be.
        expected: usize,
        /// How many were given.
        found: usize,
    },
    /// The graph would have more vertices or edges than a `u32` can count.
    TooLarge,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::VertexOutOfRange { vertex } => {
                write!(f, "vertex {vertex} is out of range")
            }
            GraphError::SelfLoop { vertex } => write!(f, "vertex {vertex} has an edge to itself"),
            GraphError::WrongCellCount { expected, found } => {
                write!(f, "expected {expected} cells, found {found}")
            }
            GraphError::TooLarge => f.write_str("the graph is too large"),
        }
    }
}

impl Error for GraphError {}

impl Graph {
    /// Builds a graph of `vertices` vertices from undirected edges.
    ///
    /// Each vertex's neighbours are kept in the order its edges are given. An edge given twice
    /// is kept once.
    ///
    /// # Errors
    ///
    /// Returns an error for an edge to a vertex out of range, or from a vertex to itself.
    pub fn from_edges(vertices: u32, edges: &[(u32, u32)]) -> Result<Self, GraphError> {
        let mut lists: Vec<Vec<u32>> = vec![Vec::new(); vertices as usize];
        for &(a, b) in edges {
            for vertex in [a, b] {
                if vertex >= vertices {
                    return Err(GraphError::VertexOutOfRange { vertex });
                }
            }
            if a == b {
                return Err(GraphError::SelfLoop { vertex: a });
            }
            if !lists[a as usize].contains(&b) {
                lists[a as usize].push(b);
                lists[b as usize].push(a);
            }
        }
        Graph::from_lists(&lists, Vec::new(), Vec::new(), 0, 0)
    }

    /// Builds the 4-connected graph of a grid's passable cells, as the MovingAI benchmarks
    /// for multi-agent path finding have it. `passable` is in row-major order, `y` from the
    /// top.
    ///
    /// Vertices number the passable cells in row-major order, and neighbours are listed up,
    /// left, right, down.
    ///
    /// # Errors
    ///
    /// Returns an error if `passable` doesn't have `width * height` cells.
    pub fn from_grid(width: u32, height: u32, passable: &[bool]) -> Result<Self, GraphError> {
        let expected = width as usize * height as usize;
        if passable.len() != expected {
            return Err(GraphError::WrongCellCount {
                expected,
                found: passable.len(),
            });
        }
        let mut vertex_of_cell = vec![NO_VERTEX; expected];
        let mut cells = Vec::new();
        for (index, _) in passable.iter().enumerate().filter(|(_, open)| **open) {
            vertex_of_cell[index] = u32::try_from(cells.len()).map_err(|_| GraphError::TooLarge)?;
            // Both fit, as `index` is below `width * height`.
            cells.push((
                (index % width as usize) as u32,
                (index / width as usize) as u32,
            ));
        }
        let lists: Vec<Vec<u32>> = cells
            .iter()
            .map(|&(x, y)| {
                let candidates = [
                    (y > 0).then(|| (x, y - 1)),
                    (x > 0).then(|| (x - 1, y)),
                    (x + 1 < width).then(|| (x + 1, y)),
                    (y + 1 < height).then(|| (x, y + 1)),
                ];
                candidates
                    .into_iter()
                    .flatten()
                    .map(|(cx, cy)| vertex_of_cell[cy as usize * width as usize + cx as usize])
                    .filter(|&vertex| vertex != NO_VERTEX)
                    .collect()
            })
            .collect();
        Graph::from_lists(&lists, cells, vertex_of_cell, width, height)
    }

    fn from_lists(
        lists: &[Vec<u32>],
        cells: Vec<(u32, u32)>,
        vertex_of_cell: Vec<u32>,
        width: u32,
        height: u32,
    ) -> Result<Self, GraphError> {
        let mut offsets = Vec::with_capacity(lists.len() + 1);
        let mut neighbours = Vec::new();
        offsets.push(0);
        for list in lists {
            neighbours.extend_from_slice(list);
            offsets.push(u32::try_from(neighbours.len()).map_err(|_| GraphError::TooLarge)?);
        }
        Ok(Graph {
            offsets,
            neighbours,
            cells,
            vertex_of_cell,
            width,
            height,
        })
    }

    /// Returns how many vertices there are.
    #[inline]
    pub fn vertex_count(&self) -> u32 {
        // Checked when built.
        (self.offsets.len() - 1) as u32
    }

    /// Returns the neighbours of `vertex`, in the order searches try them, or nothing for a
    /// vertex out of range.
    #[inline]
    pub fn neighbours(&self, vertex: u32) -> &[u32] {
        let index = vertex as usize;
        if index + 1 >= self.offsets.len() {
            return &[];
        }
        &self.neighbours[self.offsets[index] as usize..self.offsets[index + 1] as usize]
    }

    /// Returns whether `a` and `b` are joined by an edge.
    #[inline]
    pub fn adjacent(&self, a: u32, b: u32) -> bool {
        self.neighbours(a).contains(&b)
    }

    /// Returns the vertex of the cell at `(x, y)`, for a graph built from a grid, or `None`
    /// for a blocked cell, one outside the grid, or a graph that isn't a grid.
    pub fn vertex_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let vertex = self.vertex_of_cell[y as usize * self.width as usize + x as usize];
        (vertex != NO_VERTEX).then_some(vertex)
    }

    /// Returns the cell of `vertex`, for a graph built from a grid.
    pub fn cell(&self, vertex: u32) -> Option<(u32, u32)> {
        self.cells.get(vertex as usize).copied()
    }

    /// Returns the number of steps from every vertex to `to`, or `u32::MAX` for a vertex that
    /// can't reach it, by a breadth-first search.
    pub fn distances_to(&self, to: u32) -> Vec<u32> {
        let mut distance = vec![u32::MAX; self.vertex_count() as usize];
        let Some(first) = distance.get_mut(to as usize) else {
            return distance;
        };
        *first = 0;
        let mut queue = VecDeque::from([to]);
        while let Some(vertex) = queue.pop_front() {
            let next = distance[vertex as usize] + 1;
            for &neighbour in self.neighbours(vertex) {
                if distance[neighbour as usize] == u32::MAX {
                    distance[neighbour as usize] = next;
                    queue.push_back(neighbour);
                }
            }
        }
        distance
    }
}

/// An agent: where it starts, at time 0, and the vertex it has to end at and stay on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Agent {
    /// The vertex it starts on.
    pub start: u32,
    /// The vertex it ends on.
    pub goal: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_number_open_cells_in_rows_and_join_their_neighbours() {
        // .@.
        // ...
        let open = [true, false, true, true, true, true];
        let graph = Graph::from_grid(3, 2, &open).unwrap();
        assert_eq!(graph.vertex_count(), 5);
        assert_eq!(graph.vertex_at(1, 0), None);
        assert_eq!(graph.vertex_at(2, 0), Some(1));
        assert_eq!(graph.vertex_at(3, 0), None);
        assert_eq!(graph.cell(3), Some((1, 1)));
        // Up, left, right, down.
        assert_eq!(graph.neighbours(3), &[2, 4]);
        assert_eq!(graph.neighbours(4), &[1, 3]);
        assert_eq!(graph.neighbours(0), &[2]);
        assert_eq!(graph.distances_to(0), [0, 4, 1, 2, 3]);
        assert!(graph.adjacent(1, 4));
        assert!(!graph.adjacent(0, 1));
    }

    #[test]
    fn edges_are_checked_and_kept_once() {
        let graph = Graph::from_edges(3, &[(0, 1), (1, 0), (1, 2)]).unwrap();
        assert_eq!(graph.neighbours(1), &[0, 2]);
        assert_eq!(graph.neighbours(0), &[1]);
        assert_eq!(graph.neighbours(7), &[] as &[u32]);
        assert_eq!(graph.cell(0), None);
        assert_eq!(graph.vertex_at(0, 0), None);
        assert_eq!(
            Graph::from_edges(2, &[(0, 2)]),
            Err(GraphError::VertexOutOfRange { vertex: 2 })
        );
        assert_eq!(
            Graph::from_edges(2, &[(1, 1)]),
            Err(GraphError::SelfLoop { vertex: 1 })
        );
        assert_eq!(
            Graph::from_grid(2, 2, &[true]),
            Err(GraphError::WrongCellCount {
                expected: 4,
                found: 1
            })
        );
        let disconnected = Graph::from_edges(3, &[(0, 1)]).unwrap();
        assert_eq!(disconnected.distances_to(0), [0, 1, u32::MAX]);
    }
}
