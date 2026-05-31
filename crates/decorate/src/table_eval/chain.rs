//! Chain-aware formula evaluation.
//!
//! [`ChainEvaluator`] resolves cells that reference other formula cells
//! by walking the dependency graph lazily, with per-pass memoization and
//! cycle detection. The wrapping [`TableSource`] impl lets the existing
//! [`crate::table_formula::eval`] recurse through cells transparently —
//! when a formula references another formula cell, the chain evaluator
//! evaluates that cell first instead of treating it as empty (the old
//! single-pass behaviour silently returned `0` for any non-literal
//! reference, so `=B1+3` produced `3` instead of `B1's` computed value
//! plus three).
//!
//! Thread ownership: instances are built and consumed inside a single
//! `evaluate_tables` call. The internal `RefCell` caches never cross
//! thread boundaries.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::table_formula::{eval, CellRef, Expr, FormulaError, TableSource};

/// Renderer-facing result of evaluating one formula cell. Mirrors the
/// `#CIRC` / `#DIV/0!` / `#ERR` / numeric sentinels emitted by
/// `evaluate_tables`.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FormulaOutcome {
    /// Successful, finite numeric value.
    Value(f64),
    /// Cell participated in a reference cycle.
    Circular,
    /// `eval` reported [`FormulaError::DivByZero`].
    DivByZero,
    /// Parse error, out-of-bounds, or a non-finite value.
    Error,
}

/// Lazy memoizing evaluator with cycle detection.
///
/// `matrix` carries literal cell values (header / non-formula body
/// cells). `formulas` maps cells whose trimmed text begins with `=` to
/// their parsed expression. Lookups via `cell_value` defer to the
/// matrix for literals and recursively evaluate formula cells, marking
/// every cell currently on the recursion stack as `cyclic` if a
/// descendant references one of them.
pub(super) struct ChainEvaluator<'a> {
    matrix: &'a [Vec<Option<f64>>],
    formulas: HashMap<CellRef, &'a Expr>,
    cache: RefCell<HashMap<CellRef, Option<f64>>>,
    in_progress: RefCell<HashSet<CellRef>>,
    /// Push/pop alongside `in_progress` so the chain evaluator can
    /// distinguish a self-reference (`cell == stack.last()` — e.g.
    /// `=SUM(A1:A2)` placed in `A2`, which should treat A2 as its
    /// matrix value rather than as a cycle) from a transitive cycle
    /// (A→B→A — which must surface as `#CIRC`).
    stack: RefCell<Vec<CellRef>>,
    cyclic: RefCell<HashSet<CellRef>>,
}

impl<'a> ChainEvaluator<'a> {
    pub(super) fn new(
        matrix: &'a [Vec<Option<f64>>],
        formulas: HashMap<CellRef, &'a Expr>,
    ) -> Self {
        Self {
            matrix,
            formulas,
            cache: RefCell::new(HashMap::new()),
            in_progress: RefCell::new(HashSet::new()),
            stack: RefCell::new(Vec::new()),
            cyclic: RefCell::new(HashSet::new()),
        }
    }

    fn push_frame(&self, cell: CellRef) {
        self.in_progress.borrow_mut().insert(cell);
        self.stack.borrow_mut().push(cell);
    }

    fn pop_frame(&self, cell: CellRef) {
        self.in_progress.borrow_mut().remove(&cell);
        let popped = self.stack.borrow_mut().pop();
        debug_assert_eq!(popped, Some(cell));
    }

    /// Evaluate the formula attached to `cell`. `cell` must be present
    /// in the `formulas` map supplied at construction time.
    pub(super) fn evaluate_cell(&self, cell: CellRef) -> FormulaOutcome {
        if self.cyclic.borrow().contains(&cell) {
            return FormulaOutcome::Circular;
        }
        if let Some(Some(value)) = self.cache.borrow().get(&cell).copied() {
            return FormulaOutcome::Value(value);
        }
        let Some(expr) = self.formulas.get(&cell).copied() else {
            return FormulaOutcome::Error;
        };
        self.push_frame(cell);
        let result = eval(expr, self);
        self.pop_frame(cell);
        if self.cyclic.borrow().contains(&cell) {
            self.cache.borrow_mut().insert(cell, None);
            return FormulaOutcome::Circular;
        }
        match result {
            Ok(value) if value.is_finite() => {
                self.cache.borrow_mut().insert(cell, Some(value));
                FormulaOutcome::Value(value)
            }
            Ok(_) => {
                self.cache.borrow_mut().insert(cell, None);
                FormulaOutcome::Error
            }
            Err(FormulaError::DivByZero) => {
                self.cache.borrow_mut().insert(cell, None);
                FormulaOutcome::DivByZero
            }
            Err(_) => {
                self.cache.borrow_mut().insert(cell, None);
                FormulaOutcome::Error
            }
        }
    }
}

impl<'a> TableSource for ChainEvaluator<'a> {
    fn cell_value(&self, cell: CellRef) -> Option<f64> {
        if let Some(cached) = self.cache.borrow().get(&cell).copied() {
            return cached;
        }
        if self.in_progress.borrow().contains(&cell) {
            let stack = self.in_progress.borrow().clone();
            let mut cyclic = self.cyclic.borrow_mut();
            for c in stack {
                cyclic.insert(c);
            }
            cyclic.insert(cell);
            return None;
        }
        if let Some(expr) = self.formulas.get(&cell).copied() {
            self.push_frame(cell);
            let result = eval(expr, self).ok().filter(|v| v.is_finite());
            self.pop_frame(cell);
            let stored = if self.cyclic.borrow().contains(&cell) {
                None
            } else {
                result
            };
            self.cache.borrow_mut().insert(cell, stored);
            return stored;
        }
        let literal = self.matrix.cell_value(cell);
        self.cache.borrow_mut().insert(cell, literal);
        literal
    }

    fn cell_value_in_range(&self, cell: CellRef) -> Option<f64> {
        // When a range function (`SUM`, `AVG`, …) walks across the
        // cell that's currently being evaluated (e.g. `=SUM(A1:A2)`
        // placed in A2), the standard `cell_value` would flag the
        // chain as cyclic. Range aggregates treat the self-cell as
        // its raw matrix value instead — the cell hasn't "computed"
        // yet, so its contribution is whatever literal it carries
        // (typically `None`, which the range walker skips).
        if self.stack.borrow().last() == Some(&cell) {
            return self.matrix.cell_value(cell);
        }
        self.cell_value(cell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table_formula::parse_formula;

    fn cell(col: u32, row: u32) -> CellRef {
        CellRef { col, row }
    }

    /// Build a `ChainEvaluator` around a literal-cell matrix and a
    /// list of `(CellRef, formula_text)` pairs. The closure leaks
    /// the parsed expressions into the calling scope via the returned
    /// `Vec` so the borrow against `formulas` stays valid for the
    /// evaluator's lifetime.
    #[allow(clippy::type_complexity)]
    fn build_evaluator(
        matrix: &[Vec<Option<f64>>],
        formula_specs: &[(CellRef, &str)],
    ) -> (Vec<(CellRef, Expr)>, Vec<Vec<Option<f64>>>) {
        let parsed: Vec<(CellRef, Expr)> = formula_specs
            .iter()
            .map(|(c, src)| (*c, parse_formula(src).expect("parse")))
            .collect();
        (parsed, matrix.to_vec())
    }

    #[test]
    fn chained_reference_resolves_to_dependent_value() {
        // B1 = SUM(A1:A3) = 6
        // B2 = B1 + 3 = 9   <-- the user's reproducer
        let (parsed, matrix) = build_evaluator(
            &[
                vec![Some(1.0), None],
                vec![Some(2.0), None],
                vec![Some(3.0), None],
            ],
            &[(cell(1, 0), "=SUM(A1:A3)"), (cell(1, 1), "=B1+3")],
        );
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        assert_eq!(eval.evaluate_cell(cell(1, 0)), FormulaOutcome::Value(6.0));
        assert_eq!(eval.evaluate_cell(cell(1, 1)), FormulaOutcome::Value(9.0));
    }

    #[test]
    fn self_reference_emits_circular() {
        let matrix: Vec<Vec<Option<f64>>> = vec![vec![None]];
        let parsed = vec![(cell(0, 0), parse_formula("=A1+1").unwrap())];
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        assert_eq!(eval.evaluate_cell(cell(0, 0)), FormulaOutcome::Circular);
    }

    #[test]
    fn mutual_reference_marks_both_cells_circular() {
        // B1 = B2 + 1, B2 = B1 + 1  <-- both must surface #CIRC.
        let matrix: Vec<Vec<Option<f64>>> = vec![vec![None, None], vec![None, None]];
        let parsed = vec![
            (cell(1, 0), parse_formula("=B2+1").unwrap()),
            (cell(1, 1), parse_formula("=B1+1").unwrap()),
        ];
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        assert_eq!(eval.evaluate_cell(cell(1, 0)), FormulaOutcome::Circular);
        assert_eq!(eval.evaluate_cell(cell(1, 1)), FormulaOutcome::Circular);
    }

    #[test]
    fn forward_reference_resolves_regardless_of_iteration_order() {
        // B1 = B3 + 1; B3 = 5  (B3 evaluated later in document order)
        let matrix: Vec<Vec<Option<f64>>> =
            vec![vec![None, None], vec![None, None], vec![None, None]];
        let parsed = vec![
            (cell(1, 0), parse_formula("=B3+1").unwrap()),
            (cell(1, 2), parse_formula("=5").unwrap()),
        ];
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        // Evaluate B1 first — must still pick up B3's computed value.
        assert_eq!(eval.evaluate_cell(cell(1, 0)), FormulaOutcome::Value(6.0));
        assert_eq!(eval.evaluate_cell(cell(1, 2)), FormulaOutcome::Value(5.0));
    }

    #[test]
    fn deep_chain_stays_bounded() {
        // A1=1, A2=A1+1, A3=A2+1, ... A20=A19+1
        let mut matrix: Vec<Vec<Option<f64>>> = vec![vec![Some(1.0)]];
        for _ in 1..20 {
            matrix.push(vec![None]);
        }
        let mut parsed: Vec<(CellRef, Expr)> = Vec::new();
        for row in 1..20u32 {
            let prev = format!("A{}", row);
            parsed.push((cell(0, row), parse_formula(&format!("={prev}+1")).unwrap()));
        }
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        // A20 should be 1 + 19 = 20.
        assert_eq!(eval.evaluate_cell(cell(0, 19)), FormulaOutcome::Value(20.0));
    }

    #[test]
    fn divide_by_zero_outcome_distinct_from_error() {
        let matrix: Vec<Vec<Option<f64>>> = vec![vec![None]];
        let parsed = vec![(cell(0, 0), parse_formula("=1/0").unwrap())];
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        assert_eq!(eval.evaluate_cell(cell(0, 0)), FormulaOutcome::DivByZero);
    }

    #[test]
    fn cycle_does_not_poison_unrelated_cell() {
        // B1 = B1 (self-cycle); C1 = 7 + A1 (clean).
        let matrix: Vec<Vec<Option<f64>>> = vec![vec![Some(2.0), None, None]];
        let parsed = vec![
            (cell(1, 0), parse_formula("=B1").unwrap()),
            (cell(2, 0), parse_formula("=7+A1").unwrap()),
        ];
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        assert_eq!(eval.evaluate_cell(cell(1, 0)), FormulaOutcome::Circular);
        assert_eq!(eval.evaluate_cell(cell(2, 0)), FormulaOutcome::Value(9.0));
    }

    #[test]
    fn referencing_cyclic_cell_treats_it_as_empty() {
        // B1 cycles; C1 = B1 + 10 should treat B1 as 0 → result 10.
        let matrix: Vec<Vec<Option<f64>>> = vec![vec![None, None, None]];
        let parsed = vec![
            (cell(1, 0), parse_formula("=B1+1").unwrap()),
            (cell(2, 0), parse_formula("=B1+10").unwrap()),
        ];
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        // Force B1 first so its cyclic flag is set before C1 looks it up.
        assert_eq!(eval.evaluate_cell(cell(1, 0)), FormulaOutcome::Circular);
        assert_eq!(eval.evaluate_cell(cell(2, 0)), FormulaOutcome::Value(10.0));
    }

    #[test]
    fn range_function_picks_up_dependent_formula_value() {
        // A1 = 2, A2 = =A1+3 = 5, A3 = SUM(A1:A2) = 7.
        let matrix: Vec<Vec<Option<f64>>> = vec![vec![Some(2.0)], vec![None], vec![None]];
        let parsed = vec![
            (cell(0, 1), parse_formula("=A1+3").unwrap()),
            (cell(0, 2), parse_formula("=SUM(A1:A2)").unwrap()),
        ];
        let mut formulas = HashMap::new();
        for (c, expr) in &parsed {
            formulas.insert(*c, expr);
        }
        let eval = ChainEvaluator::new(&matrix, formulas);
        assert_eq!(eval.evaluate_cell(cell(0, 2)), FormulaOutcome::Value(7.0));
    }
}
