//! Parsing for the ASCII observation grids used by the bedrock and terrain
//! modes.
//!
//! The user transcribes what they can see from a screenshot into a small text
//! grid. Cells they cannot make out are marked unknown rather than guessed,
//! because a single wrong cell silently eliminates the correct answer.

use anyhow::{Result, bail};

/// One cell of a two-state (present/absent/unknown) grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// `#` — the feature is present (bedrock).
    Present,
    /// `.` — the feature is definitely absent (air, lava, netherrack…).
    Absent,
    /// `?` — not observable; contributes no constraint.
    Unknown,
}

/// A rectangular grid of observations, indexed `[row][col]` where row advances
/// along +Z and column along +X.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    pub cells: Vec<Vec<Cell>>,
}

impl Grid {
    pub fn width(&self) -> usize {
        self.cells.first().map(|r| r.len()).unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.cells.len()
    }

    /// Number of cells that actually constrain anything.
    pub fn known_count(&self) -> usize {
        self.cells
            .iter()
            .flatten()
            .filter(|c| **c != Cell::Unknown)
            .count()
    }

    /// Iterates `(dx, dz, cell)` for every non-unknown cell, with offsets
    /// relative to the grid's top-left corner.
    pub fn known_cells(&self) -> impl Iterator<Item = (i32, i32, Cell)> + '_ {
        self.cells.iter().enumerate().flat_map(|(row, cells)| {
            cells
                .iter()
                .enumerate()
                .filter(|(_, c)| **c != Cell::Unknown)
                .map(move |(col, c)| (col as i32, row as i32, *c))
        })
    }

    /// Parses lines of `#`, `.` and `?`.
    ///
    /// Rows are padded to the widest row with `Unknown`, so a ragged paste
    /// still works rather than being rejected outright.
    pub fn parse(lines: &[String]) -> Result<Grid> {
        let mut rows: Vec<Vec<Cell>> = Vec::new();

        for line in lines {
            let trimmed = line.trim_end();
            if trimmed.trim().is_empty() {
                continue;
            }
            let mut row = Vec::new();
            for ch in trimmed.chars() {
                match ch {
                    '#' | 'B' | 'b' | 'X' | 'x' | '1' => row.push(Cell::Present),
                    '.' | '_' | '0' | 'O' | 'o' => row.push(Cell::Absent),
                    '?' | ' ' | '-' => row.push(Cell::Unknown),
                    other => bail!(
                        "unexpected character {other:?} in grid; use '#' for bedrock, \
                         '.' for not-bedrock and '?' for unknown"
                    ),
                }
            }
            rows.push(row);
        }

        if rows.is_empty() {
            bail!("the grid is empty");
        }

        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for row in &mut rows {
            row.resize(width, Cell::Unknown);
        }

        let grid = Grid { cells: rows };
        if grid.known_count() == 0 {
            bail!("the grid contains no known cells, so it constrains nothing");
        }
        Ok(grid)
    }

    /// Loads a grid from a text file.
    pub fn from_file(path: &str) -> Result<Grid> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("could not read {path}: {e}"))?;
        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        Grid::parse(&lines)
    }

    pub fn render(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| match c {
                        Cell::Present => '#',
                        Cell::Absent => '.',
                        Cell::Unknown => '?',
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A grid of numeric values with optional holes, used by the terrain matcher.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueGrid {
    pub cells: Vec<Vec<Option<f64>>>,
}

impl ValueGrid {
    pub fn width(&self) -> usize {
        self.cells.first().map(|r| r.len()).unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.cells.len()
    }

    pub fn known_count(&self) -> usize {
        self.cells.iter().flatten().filter(|c| c.is_some()).count()
    }

    /// Parses whitespace-separated numbers, with `?` marking a hole.
    pub fn parse(lines: &[String]) -> Result<ValueGrid> {
        let mut rows: Vec<Vec<Option<f64>>> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let mut row = Vec::new();
            for tok in line.split_whitespace() {
                if tok == "?" || tok == "-" {
                    row.push(None);
                } else {
                    match tok.parse::<f64>() {
                        Ok(v) => row.push(Some(v)),
                        Err(_) => bail!("{tok:?} is not a number (use '?' for unknown)"),
                    }
                }
            }
            rows.push(row);
        }

        if rows.is_empty() {
            bail!("the grid is empty");
        }
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for row in &mut rows {
            row.resize(width, None);
        }

        let grid = ValueGrid { cells: rows };
        if grid.known_count() == 0 {
            bail!("the grid contains no known values");
        }
        Ok(grid)
    }

    pub fn from_file(path: &str) -> Result<ValueGrid> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("could not read {path}: {e}"))?;
        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        ValueGrid::parse(&lines)
    }

    pub fn known_cells(&self) -> impl Iterator<Item = (i32, i32, f64)> + '_ {
        self.cells.iter().enumerate().flat_map(|(row, cells)| {
            cells
                .iter()
                .enumerate()
                .filter_map(move |(col, c)| c.map(|v| (col as i32, row as i32, v)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_grid() {
        let lines: Vec<String> = ["#.#", ".?#", "###"].iter().map(|s| s.to_string()).collect();
        let g = Grid::parse(&lines).unwrap();
        assert_eq!(g.width(), 3);
        assert_eq!(g.height(), 3);
        assert_eq!(g.known_count(), 8);
        assert_eq!(g.cells[0][0], Cell::Present);
        assert_eq!(g.cells[0][1], Cell::Absent);
        assert_eq!(g.cells[1][1], Cell::Unknown);
    }

    #[test]
    fn ragged_rows_are_padded_with_unknowns() {
        let lines: Vec<String> = ["##", "#"].iter().map(|s| s.to_string()).collect();
        let g = Grid::parse(&lines).unwrap();
        assert_eq!(g.width(), 2);
        assert_eq!(g.cells[1][1], Cell::Unknown);
    }

    #[test]
    fn rejects_grids_that_constrain_nothing() {
        let lines: Vec<String> = ["???", "???"].iter().map(|s| s.to_string()).collect();
        assert!(Grid::parse(&lines).is_err());
        assert!(Grid::parse(&[]).is_err());
    }

    #[test]
    fn rejects_unexpected_characters() {
        let lines: Vec<String> = vec!["#$#".to_string()];
        assert!(Grid::parse(&lines).is_err());
    }

    #[test]
    fn round_trips_through_render() {
        let lines: Vec<String> = ["#.?", ".#."].iter().map(|s| s.to_string()).collect();
        let g = Grid::parse(&lines).unwrap();
        assert_eq!(g.render(), "#.?\n.#.");
    }

    #[test]
    fn known_cells_reports_offsets() {
        let lines: Vec<String> = vec!["?#".to_string()];
        let g = Grid::parse(&lines).unwrap();
        let found: Vec<_> = g.known_cells().collect();
        assert_eq!(found, vec![(1, 0, Cell::Present)]);
    }

    #[test]
    fn value_grid_parses_numbers_and_holes() {
        let lines: Vec<String> = vec!["70 71 ?".to_string(), "72 ? 74".to_string()];
        let g = ValueGrid::parse(&lines).unwrap();
        assert_eq!(g.width(), 3);
        assert_eq!(g.known_count(), 4);
        assert_eq!(g.cells[0][0], Some(70.0));
        assert_eq!(g.cells[0][2], None);
    }
}
