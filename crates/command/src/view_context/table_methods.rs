macro_rules! view_context_table_methods {
    () => {
        /// Insert a blank row immediately above (`above=true`) or below
        /// the caret's current row inside a pipe-table block. No-op
        /// (`UnsupportedContext`) when the caret isn't inside a table.
        fn markdown_table_insert_row(&mut self, _above: bool) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_insert_row"))
        }

        /// Insert a blank column immediately left (`before=true`) or right
        /// of the caret's current column inside a pipe-table block.
        fn markdown_table_insert_column(&mut self, _before: bool) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_insert_column"))
        }

        /// Delete the row containing the caret inside a pipe-table block.
        /// Refuses to delete the header row or the alignment row.
        fn markdown_table_delete_row(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_delete_row"))
        }

        /// Delete the column containing the caret inside a pipe-table
        /// block. Refuses to delete the last remaining column.
        fn markdown_table_delete_column(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_delete_column"))
        }

        /// Delete the entire pipe-table block containing the caret.
        fn markdown_table_delete_table(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_delete_table"))
        }

        /// Select the entire content of every cell that currently contains
        /// a caret. Bound to Ctrl+A when `editor.in_table` so the in-cell
        /// "select all" scopes to the cell instead of the whole buffer.
        fn markdown_table_select_cell(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_select_cell"))
        }

        /// Move (or extend, when `extend = true`) every cell-containing
        /// caret to that cell's content edge. `to_start = true` jumps to
        /// the cell's start, `false` to its end. Carets outside cells
        /// fall through to default behavior (the dispatcher returns
        /// `UnsupportedContext` in that case so the keymap layer can
        /// retry the global binding).
        fn markdown_table_caret_cell_edge(
            &mut self,
            _to_start: bool,
            _extend: bool,
        ) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_caret_cell_edge"))
        }

        /// Cell-aware Tab. Moves the primary caret to the next cell
        /// (left→right; wraps to the next row's first cell at row end).
        /// When the caret sits in the last cell of the last body row,
        /// inserts a new blank body row below and lands the caret in its
        /// first cell. Returns `UnsupportedContext` when no caret is in a
        /// table so the keymap falls through to the global Tab binding.
        fn markdown_table_tab_next(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_tab_next"))
        }

        /// Cell-aware Shift+Tab. Moves the primary caret to the previous
        /// cell (right→left; wraps to the previous row's last cell). At
        /// the first cell of the first body row, no-ops (no auto-prepend).
        /// Returns `UnsupportedContext` when no caret is in a table.
        fn markdown_table_tab_prev(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_tab_prev"))
        }

        /// Cell-aware Enter. Moves the primary caret to the cell directly
        /// below in the same column (skipping the alignment row); at the
        /// last body row, inserts a new blank row below and lands the
        /// caret in the new row's same column. Returns `UnsupportedContext`
        /// when no caret is in a table.
        fn markdown_table_enter(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_enter"))
        }

        /// Cell-aware Ctrl+Enter. Inserts the literal `<br>` HTML token at
        /// the caret position; visual line-break behavior inside the cell
        /// is deferred to Phase F (variable per-row height). Until then the
        /// token shows as text inside the cell. Returns `UnsupportedContext`
        /// when no caret is in a table.
        fn markdown_table_insert_break(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_insert_break"))
        }

        /// Cell-aware vertical motion. `down = true` jumps to the cell
        /// directly below (same column, skipping the alignment row);
        /// `down = false` jumps above. Falls through with
        /// `UnsupportedContext` at the top/bottom edge of the table or
        /// when no caret is in any cell, letting the global Up/Down
        /// binding run for everything else.
        fn markdown_table_move_vertical(&mut self, _down: bool) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_move_vertical"))
        }

        /// Cell-aware Shift+Enter. Moves the caret to the cell directly
        /// above (same column, skipping the alignment row). Unlike
        /// [`Self::markdown_table_move_vertical`], it never exits the table:
        /// at the header row it stays put, and it never falls through to a
        /// raw newline that would split the table.
        fn markdown_table_cell_up(&mut self) -> Result<(), Error> {
            Err(Error::UnsupportedContext("markdown_table_cell_up"))
        }
    };
}
