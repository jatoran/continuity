//! Phase F4 — inline table formulas.
//!
//! Tiny formula language that recomputes a cell's display value from the
//! surrounding table's cells. The source bytes stay plain markdown — the
//! formula text (`=SUM(B1:B5)` etc.) lives inside the cell verbatim; the
//! renderer-side recompute swaps in the result so the cell renders as
//! the computed number while still exporting byte-exact markdown.
//!
//! Supported surface (per `roadmap_v2.md §F4`):
//!
//! - Function calls over a rectangular range: `SUM`, `AVG`, `COUNT`,
//!   `MIN`, `MAX` taking one `A1:B3` range argument.
//! - Simple arithmetic: numbers, cell references, parentheses, and the
//!   four binary operators `+ - * /` with standard precedence.
//! - No conditionals, no string functions, no cross-buffer references.
//!
//! Cell references follow the spreadsheet convention: column letters
//! `A..Z, AA..` are 0-indexed (A → 0), row digits are 1-indexed (1 → 0).
//!
//! Thread ownership: stateless, callable from any thread.

use std::fmt;

/// A1-style cell reference. `col` and `row` are both **0-indexed** in
/// the internal representation; the parser converts from the
/// 1-indexed display form (`A1` → `col=0, row=0`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct CellRef {
    /// Zero-indexed column.
    pub col: u32,
    /// Zero-indexed row.
    pub row: u32,
}

/// Built-in range function. One range argument; returns a single
/// `f64`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FuncKind {
    /// `SUM(A1:B3)` — sum of numeric cells in the range.
    Sum,
    /// `AVG(A1:B3)` — arithmetic mean of numeric cells (zero if no
    /// numeric cells).
    Avg,
    /// `COUNT(A1:B3)` — count of cells that parsed as a number.
    Count,
    /// `MIN(A1:B3)` — minimum numeric cell, or zero if none.
    Min,
    /// `MAX(A1:B3)` — maximum numeric cell, or zero if none.
    Max,
}

impl FuncKind {
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "SUM" => Some(Self::Sum),
            "AVG" => Some(Self::Avg),
            "COUNT" => Some(Self::Count),
            "MIN" => Some(Self::Min),
            "MAX" => Some(Self::Max),
            _ => None,
        }
    }
}

/// Binary operator.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Op {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}

/// Parsed expression tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// Numeric literal.
    Number(f64),
    /// `A1` reference.
    Ref(CellRef),
    /// Built-in function over a rectangular range.
    Range(FuncKind, CellRef, CellRef),
    /// `lhs op rhs`.
    Bin(Op, Box<Expr>, Box<Expr>),
    /// Unary minus (right-associative).
    Neg(Box<Expr>),
}

/// Errors the parser / evaluator surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormulaError {
    /// Empty input or no leading `=`.
    NoFormula,
    /// Generic syntax error with a static label for the offending token.
    Syntax(&'static str),
    /// Division by zero at evaluation time.
    DivByZero,
    /// Reference to a cell that the supplied table doesn't have.
    OutOfBounds,
}

impl fmt::Display for FormulaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFormula => write!(f, "not a formula"),
            Self::Syntax(label) => write!(f, "syntax error near {label}"),
            Self::DivByZero => write!(f, "division by zero"),
            Self::OutOfBounds => write!(f, "cell reference out of bounds"),
        }
    }
}

/// Source the evaluator queries for cell values. The table block
/// owner (the rendering layer) implements this over the parsed
/// markdown table.
pub trait TableSource {
    /// Return the numeric value of `cell`, or `None` if the cell is
    /// empty or its text doesn't parse as a number.
    fn cell_value(&self, cell: CellRef) -> Option<f64>;

    /// Lookup variant used by range-aggregate functions (`SUM`,
    /// `AVG`, ...). Defaults to [`Self::cell_value`]; implementations
    /// like [`crate::table_eval::chain::ChainEvaluator`] override it
    /// so that a range that includes the currently-evaluating cell
    /// (e.g. `=SUM(A1:A2)` placed in A2) is treated as the raw
    /// matrix value of that cell instead of surfacing `#CIRC`.
    fn cell_value_in_range(&self, cell: CellRef) -> Option<f64> {
        self.cell_value(cell)
    }
}

/// Implement [`TableSource`] over a `&[Vec<Option<f64>>]` indexed by
/// `[row][col]`. Convenient for tests and a perfectly good production
/// shape — the table parser populates a `Vec<Vec<Option<f64>>>` and
/// hands it to the evaluator.
impl TableSource for [Vec<Option<f64>>] {
    fn cell_value(&self, cell: CellRef) -> Option<f64> {
        self.get(cell.row as usize)
            .and_then(|row| row.get(cell.col as usize))
            .copied()
            .flatten()
    }
}

/// Parse a formula string (with or without the leading `=`).
///
/// # Errors
/// Returns [`FormulaError::NoFormula`] for an empty input,
/// [`FormulaError::Syntax`] for anything else that doesn't tokenize
/// or parse cleanly.
pub fn parse_formula(s: &str) -> Result<Expr, FormulaError> {
    let trimmed = s.trim().trim_start_matches('=').trim();
    if trimmed.is_empty() {
        return Err(FormulaError::NoFormula);
    }
    crate::table_formula_parser::parse(trimmed)
}

/// Evaluate `expr` against `table`.
///
/// # Errors
/// Returns [`FormulaError::DivByZero`] on `x / 0` or
/// [`FormulaError::OutOfBounds`] when a referenced cell index is
/// outside the table (a non-numeric cell is *not* an error — it just
/// contributes nothing to the result).
pub fn eval<T: TableSource + ?Sized>(expr: &Expr, table: &T) -> Result<f64, FormulaError> {
    match expr {
        Expr::Number(n) => Ok(*n),
        Expr::Ref(cell) => Ok(table.cell_value(*cell).unwrap_or(0.0)),
        Expr::Neg(inner) => Ok(-eval(inner, table)?),
        Expr::Bin(op, lhs, rhs) => {
            let l = eval(lhs, table)?;
            let r = eval(rhs, table)?;
            match op {
                Op::Add => Ok(l + r),
                Op::Sub => Ok(l - r),
                Op::Mul => Ok(l * r),
                Op::Div => {
                    if r == 0.0 {
                        Err(FormulaError::DivByZero)
                    } else {
                        Ok(l / r)
                    }
                }
            }
        }
        Expr::Range(func, start, end) => Ok(eval_range(*func, *start, *end, table)),
    }
}

fn eval_range<T: TableSource + ?Sized>(
    func: FuncKind,
    start: CellRef,
    end: CellRef,
    table: &T,
) -> f64 {
    let (c0, c1) = ord(start.col, end.col);
    let (r0, r1) = ord(start.row, end.row);
    let mut values: Vec<f64> = Vec::new();
    for r in r0..=r1 {
        for c in c0..=c1 {
            if let Some(v) = table.cell_value_in_range(CellRef { col: c, row: r }) {
                values.push(v);
            }
        }
    }
    match func {
        FuncKind::Sum => values.iter().sum(),
        FuncKind::Count => values.len() as f64,
        FuncKind::Avg => {
            if values.is_empty() {
                0.0
            } else {
                values.iter().sum::<f64>() / values.len() as f64
            }
        }
        FuncKind::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
        FuncKind::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    }
}

fn ord(a: u32, b: u32) -> (u32, u32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Build a column-aligned markdown table skeleton with `rows` data
/// rows and `cols` columns. Header row is `Col 1 | Col 2 | …`; the
/// alignment row uses `---` per column. Data cells are empty.
///
/// Used by the `markdown.insert_table` command — kept here so the
/// inline-table format stays adjacent to the formula evaluator that
/// consumes the resulting cells.
#[must_use]
pub fn format_table_skeleton(rows: u32, cols: u32) -> String {
    let cols = cols.max(1) as usize;
    let rows = rows as usize;
    let headers: Vec<String> = (0..cols).map(|i| format!("Col {}", i + 1)).collect();
    // Determine the minimum cell width per column so the pipes
    // line up. We use the longer of the header label or `---`.
    let widths: Vec<usize> = headers.iter().map(|h| h.chars().count().max(3)).collect();
    let mut out = String::new();
    out.push_str("| ");
    for (i, h) in headers.iter().enumerate() {
        out.push_str(h);
        for _ in h.chars().count()..widths[i] {
            out.push(' ');
        }
        out.push_str(" |");
        if i + 1 < cols {
            out.push(' ');
        }
    }
    out.push('\n');
    out.push_str("| ");
    for (i, w) in widths.iter().enumerate() {
        for _ in 0..*w {
            out.push('-');
        }
        out.push_str(" |");
        if i + 1 < cols {
            out.push(' ');
        }
    }
    out.push('\n');
    for _ in 0..rows {
        out.push_str("| ");
        for (i, w) in widths.iter().enumerate() {
            for _ in 0..*w {
                out.push(' ');
            }
            out.push_str(" |");
            if i + 1 < cols {
                out.push(' ');
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[&[Option<f64>]]) -> Vec<Vec<Option<f64>>> {
        rows.iter().map(|r| r.to_vec()).collect()
    }

    fn cell(col: u32, row: u32) -> CellRef {
        CellRef { col, row }
    }

    #[test]
    fn parse_simple_number() {
        let e = parse_formula("=42").unwrap();
        assert_eq!(e, Expr::Number(42.0));
    }

    #[test]
    fn parse_cell_ref_resolves_a1_to_zero_zero() {
        let e = parse_formula("=A1").unwrap();
        assert_eq!(e, Expr::Ref(cell(0, 0)));
        let e = parse_formula("=B3").unwrap();
        assert_eq!(e, Expr::Ref(cell(1, 2)));
        // Two-letter columns.
        let e = parse_formula("=AA1").unwrap();
        assert_eq!(e, Expr::Ref(cell(26, 0)));
    }

    #[test]
    fn parse_arithmetic_respects_precedence() {
        // 2 + 3 * 4 = 14 (not 20)
        let e = parse_formula("=2+3*4").unwrap();
        let t = table(&[]);
        assert_eq!(eval(&e, &t[..]).unwrap(), 14.0);
    }

    #[test]
    fn parens_override_precedence() {
        let e = parse_formula("=(2+3)*4").unwrap();
        let t = table(&[]);
        assert_eq!(eval(&e, &t[..]).unwrap(), 20.0);
    }

    #[test]
    fn unary_minus_parses() {
        let e = parse_formula("=-3*2").unwrap();
        let t = table(&[]);
        assert_eq!(eval(&e, &t[..]).unwrap(), -6.0);
    }

    #[test]
    fn division_by_zero_is_error() {
        let e = parse_formula("=1/0").unwrap();
        let t = table(&[]);
        assert_eq!(eval(&e, &t[..]), Err(FormulaError::DivByZero));
    }

    #[test]
    fn sum_range_adds_numeric_cells() {
        let t = table(&[&[Some(1.0), Some(2.0)], &[Some(3.0), Some(4.0)]]);
        let e = parse_formula("=SUM(A1:B2)").unwrap();
        assert_eq!(eval(&e, &t[..]).unwrap(), 10.0);
    }

    #[test]
    fn avg_range_means_numeric_cells() {
        let t = table(&[&[Some(2.0), Some(4.0), Some(6.0)]]);
        let e = parse_formula("=AVG(A1:C1)").unwrap();
        assert_eq!(eval(&e, &t[..]).unwrap(), 4.0);
    }

    #[test]
    fn count_range_ignores_empty_cells() {
        let t = table(&[&[Some(1.0), None, Some(2.0)]]);
        let e = parse_formula("=COUNT(A1:C1)").unwrap();
        assert_eq!(eval(&e, &t[..]).unwrap(), 2.0);
    }

    #[test]
    fn min_max_pick_extremes() {
        let t = table(&[&[Some(5.0), Some(2.0), Some(9.0), Some(-1.0)]]);
        let min = parse_formula("=MIN(A1:D1)").unwrap();
        let max = parse_formula("=MAX(A1:D1)").unwrap();
        assert_eq!(eval(&min, &t[..]).unwrap(), -1.0);
        assert_eq!(eval(&max, &t[..]).unwrap(), 9.0);
    }

    #[test]
    fn range_function_handles_descending_corners() {
        // SUM(B2:A1) must equal SUM(A1:B2).
        let t = table(&[&[Some(1.0), Some(2.0)], &[Some(3.0), Some(4.0)]]);
        let asc = parse_formula("=SUM(A1:B2)").unwrap();
        let desc = parse_formula("=SUM(B2:A1)").unwrap();
        assert_eq!(eval(&asc, &t[..]).unwrap(), eval(&desc, &t[..]).unwrap());
    }

    #[test]
    fn cell_ref_falls_back_to_zero_for_empty() {
        let t = table(&[&[None, Some(7.0)]]);
        let e = parse_formula("=A1+B1").unwrap();
        assert_eq!(eval(&e, &t[..]).unwrap(), 7.0);
    }

    #[test]
    fn parse_rejects_bad_function_name() {
        assert!(matches!(
            parse_formula("=BOGUS(A1:B2)"),
            Err(FormulaError::Syntax(_))
        ));
    }

    #[test]
    fn parse_rejects_empty_input() {
        assert_eq!(parse_formula(""), Err(FormulaError::NoFormula));
        assert_eq!(parse_formula("="), Err(FormulaError::NoFormula));
    }

    #[test]
    fn parse_accepts_with_or_without_leading_equals() {
        assert_eq!(
            parse_formula("=1+1").unwrap(),
            parse_formula("1+1").unwrap()
        );
    }

    #[test]
    fn format_table_skeleton_aligns_columns() {
        let s = format_table_skeleton(2, 3);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 4); // header + separator + 2 rows
                                    // Every line should begin and end with `|`.
        for ln in &lines {
            assert!(ln.starts_with("| "));
            assert!(ln.ends_with(" |"));
        }
        // Header row mentions the column labels.
        assert!(lines[0].contains("Col 1"));
        assert!(lines[0].contains("Col 3"));
        // Separator row is dashes.
        assert!(lines[1].contains("---"));
    }

    #[test]
    fn format_table_skeleton_clamps_cols_to_one() {
        let s = format_table_skeleton(1, 0);
        assert!(s.contains("Col 1"));
    }
}
