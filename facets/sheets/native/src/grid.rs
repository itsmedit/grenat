//! A sheet's used range, within a budget of cells.
//!
//! A sheet's rows are the box from its first non-empty row and column to
//! its last: their size is the box's area, not the file's. A workbook of a
//! few kilobytes can hold two cells, at A1 and at XFD1048576, whose box
//! has 17 billion cells — far more memory than any machine has. So the box
//! is measured before anything is built, and a sheet that spreads over more
//! than [`MAX_CELLS`] cells is refused: [`Grid`] gathers cells one by one
//! (the box growing with them), [`check`] measures a box already read.

/// The most cells a sheet's used range may hold: ten million (a thousand
/// columns of ten thousand rows, ten columns of a million rows).
pub const MAX_CELLS: u64 = 10_000_000;

/// The cells of a sheet, gathered one by one: their texts and positions
/// (row, column, from 0), and the box they span.
pub struct Grid {
    budget: u64,
    cells: Vec<((u32, u32), String)>,
    start: (u32, u32),
    end: (u32, u32),
}

impl Grid {
    /// An empty grid that refuses to span more than `budget` cells.
    pub fn new(budget: u64) -> Self {
        Grid { budget, cells: Vec::new(), start: (u32::MAX, u32::MAX), end: (0, 0) }
    }

    /// Adds the cell at `pos` (row, column); an error once the box the
    /// cells span, or their number, goes past the budget.
    pub fn add(&mut self, pos: (u32, u32), text: String) -> Result<(), String> {
        let start = (self.start.0.min(pos.0), self.start.1.min(pos.1));
        let end = (self.end.0.max(pos.0), self.end.1.max(pos.1));
        let (rows, columns) = (u64::from(end.0 - start.0) + 1, u64::from(end.1 - start.1) + 1);
        check_with(rows, columns, self.budget, " at least")?;
        if self.cells.len() as u64 >= self.budget {
            return Err(format!("it has more than {} cells", self.budget));
        }
        (self.start, self.end) = (start, end);
        self.cells.push((pos, text));
        Ok(())
    }

    /// The rows of the box, every cell not added being `""`; a cell added
    /// twice keeps its last text.
    pub fn rows(self) -> Vec<Vec<String>> {
        if self.cells.is_empty() {
            return Vec::new();
        }
        let width = (self.end.1 - self.start.1) as usize + 1;
        let height = (self.end.0 - self.start.0) as usize + 1;
        let mut rows = vec![vec![String::new(); width]; height];
        for ((row, column), text) in self.cells {
            rows[(row - self.start.0) as usize][(column - self.start.1) as usize] = text;
        }
        rows
    }
}

/// Whether a box of `rows` rows of `columns` columns fits in `budget`
/// cells ([`MAX_CELLS`] for a sheet).
pub fn check(rows: usize, columns: usize, budget: u64) -> Result<(), String> {
    check_with(rows as u64, columns as u64, budget, "")
}

/// `rows` of `columns` within `budget`; `at_least` qualifies the size of
/// a box still growing.
fn check_with(rows: u64, columns: u64, budget: u64, at_least: &str) -> Result<(), String> {
    if rows.saturating_mul(columns) > budget {
        return Err(format!(
            "its cells spread over {rows} rows of {columns} columns{at_least}: more than the {budget} cells a sheet may span"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter().map(|row| row.iter().map(|c| c.to_string()).collect()).collect()
    }

    #[test]
    fn cells_make_the_rows_of_their_box() {
        let mut grid = Grid::new(100);
        grid.add((3, 2), "c".into()).unwrap();
        grid.add((2, 1), "a".into()).unwrap();
        grid.add((4, 1), "x".into()).unwrap();
        grid.add((4, 1), "b".into()).unwrap();
        assert_eq!(grid.rows(), rows(&[&["a", ""], &["", "c"], &["b", ""]]));
        assert_eq!(Grid::new(100).rows(), Vec::<Vec<String>>::new());
        let mut one = Grid::new(1);
        one.add((7, 9), "only".into()).unwrap();
        assert_eq!(one.rows(), rows(&[&["only"]]));
    }

    #[test]
    fn a_box_past_the_budget_is_refused_as_soon_as_it_grows() {
        let mut grid = Grid::new(6);
        grid.add((0, 0), "a".into()).unwrap();
        grid.add((1, 2), "b".into()).unwrap();
        let e = grid.add((2, 0), "c".into()).unwrap_err();
        assert_eq!(e, "its cells spread over 3 rows of 3 columns at least: more than the 6 cells a sheet may span");
        // the refused cell is not kept
        assert_eq!(grid.rows(), rows(&[&["a", "", ""], &["", "", "b"]]));
        // the farthest cells of a sheet: no overflow
        let mut far = Grid::new(MAX_CELLS);
        far.add((0, 0), "a".into()).unwrap();
        assert!(far.add((1_048_575, 16_383), "z".into()).unwrap_err().contains("1048576 rows of 16384 columns"));
    }

    #[test]
    fn cells_added_again_and_again_are_refused_too() {
        let mut grid = Grid::new(2);
        grid.add((0, 0), "a".into()).unwrap();
        grid.add((0, 0), "a".into()).unwrap();
        assert_eq!(grid.add((0, 0), "a".into()).unwrap_err(), "it has more than 2 cells");
    }

    #[test]
    fn a_box_read_already_is_measured() {
        assert_eq!(check(10_000, 1_000, MAX_CELLS), Ok(()));
        assert_eq!(check(0, 0, 0), Ok(()));
        assert_eq!(check(2, 3, 6), Ok(()));
        assert!(check(2, 4, 6).is_err());
        let e = check(2000, 16_384, MAX_CELLS).unwrap_err();
        assert_eq!(
            e,
            "its cells spread over 2000 rows of 16384 columns: more than the 10000000 cells a sheet may span"
        );
        assert!(check(usize::MAX, usize::MAX, MAX_CELLS).is_err());
    }
}
