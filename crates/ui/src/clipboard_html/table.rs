//! GFM pipe-table rendering for the HTML-to-markdown paste converter.
//!
//! Split out of [`crate::clipboard_html`] to keep that module under the
//! 600-line cap. Pure functions; no shared state.

use super::HtmlNode;

/// Render a `<table>` [`HtmlNode`] as a GFM pipe table string, or `None`
/// when the element has no rows.
///
/// `render_cell` converts a cell's inline children to a single-line
/// markdown string (the converter supplies one that runs the full inline
/// renderer). Rows are gathered from any nested `<thead>`/`<tbody>`/
/// `<tfoot>`/`<tr>`; the first row becomes the header and a delimiter row
/// is synthesized beneath it. Ragged rows are padded to the widest row.
pub(crate) fn render_table(
    node: &HtmlNode,
    render_cell: &mut dyn FnMut(&[HtmlNode]) -> String,
) -> Option<String> {
    let HtmlNode::Element { children, .. } = node else {
        return None;
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    collect_table_rows(children, render_cell, &mut rows);
    if rows.is_empty() {
        return None;
    }
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return None;
    }
    for row in &mut rows {
        while row.len() < cols {
            row.push(String::new());
        }
    }
    let mut out = String::new();
    out.push_str(&format_table_row(&rows[0]));
    out.push('\n');
    out.push('|');
    for _ in 0..cols {
        out.push_str(" --- |");
    }
    for row in &rows[1..] {
        out.push('\n');
        out.push_str(&format_table_row(row));
    }
    Some(out)
}

/// Recursively gather `<tr>` rows from a table's children, descending
/// through `<thead>`/`<tbody>`/`<tfoot>` section wrappers.
fn collect_table_rows(
    nodes: &[HtmlNode],
    render_cell: &mut dyn FnMut(&[HtmlNode]) -> String,
    rows: &mut Vec<Vec<String>>,
) {
    for node in nodes {
        let HtmlNode::Element { tag, children, .. } = node else {
            continue;
        };
        match tag.as_str() {
            "thead" | "tbody" | "tfoot" => collect_table_rows(children, render_cell, rows),
            "tr" => {
                let mut cells = Vec::new();
                for cell in children {
                    if let HtmlNode::Element {
                        tag: cell_tag,
                        children: cell_children,
                        ..
                    } = cell
                    {
                        if cell_tag == "td" || cell_tag == "th" {
                            cells.push(render_cell(cell_children));
                        }
                    }
                }
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
            _ => collect_table_rows(children, render_cell, rows),
        }
    }
}

/// Format one table row as `| a | b | c |`, escaping literal pipes inside
/// cells so they don't add phantom columns.
fn format_table_row(cells: &[String]) -> String {
    let mut out = String::from("|");
    for cell in cells {
        out.push(' ');
        out.push_str(&cell.replace('|', "\\|"));
        out.push(' ');
        out.push('|');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard_html::parse_html;

    /// Render a table from HTML, using each cell's raw concatenated text
    /// as the cell content (keeps the test independent of the full inline
    /// renderer).
    fn render(html: &str) -> Option<String> {
        let nodes = parse_html(html);
        let table = nodes
            .iter()
            .find(|n| matches!(n, HtmlNode::Element { tag, .. } if tag == "table"))?;
        render_table(table, &mut |children| {
            let mut out = String::new();
            collect_cell_text(children, &mut out);
            out.trim().to_string()
        })
    }

    fn collect_cell_text(nodes: &[HtmlNode], out: &mut String) {
        for node in nodes {
            match node {
                HtmlNode::Text(t) => out.push_str(t),
                HtmlNode::Element { children, .. } => collect_cell_text(children, out),
            }
        }
    }

    #[test]
    fn simple_table_to_gfm() {
        let html = "<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>";
        let out = render(html).expect("table");
        assert_eq!(out, "| A | B |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn table_with_sections() {
        let html =
            "<table><thead><tr><th>H</th></tr></thead><tbody><tr><td>x</td></tr></tbody></table>";
        let out = render(html).expect("table");
        assert_eq!(out, "| H |\n| --- |\n| x |");
    }

    #[test]
    fn ragged_rows_padded() {
        let html = "<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>";
        let out = render(html).expect("table");
        assert_eq!(out, "| a | b |\n| --- | --- |\n| c |  |");
    }

    #[test]
    fn pipe_in_cell_escaped() {
        let html = "<table><tr><td>a|b</td></tr></table>";
        let out = render(html).expect("table");
        assert_eq!(out, "| a\\|b |\n| --- |");
    }

    #[test]
    fn empty_table_is_none() {
        assert_eq!(render("<table></table>"), None);
        assert_eq!(render("<table><tr></tr></table>"), None);
    }
}
