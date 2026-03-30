//! Terminal text selection helpers: coordinate mapping, word boundaries,
//! and text extraction from wezterm-term screen state.

use amux_term::pane::TerminalPane;
use managed_pane::WORD_DELIMITERS;

use crate::managed_pane;

/// Convert a pointer position to terminal cell coordinates (col, stable_row).
pub(crate) fn pointer_to_cell(
    pointer_pos: egui::Pos2,
    content_rect: egui::Rect,
    cell_width: f32,
    cell_height: f32,
    scroll_offset: usize,
    total_rows: usize,
    visible_rows: usize,
) -> (usize, usize) {
    let col = ((pointer_pos.x - content_rect.min.x) / cell_width)
        .floor()
        .max(0.0) as usize;
    let visible_row = ((pointer_pos.y - content_rect.min.y) / cell_height)
        .floor()
        .max(0.0) as usize;
    let visible_row = visible_row.min(visible_rows.saturating_sub(1));
    let stable_row = total_rows
        .saturating_sub(visible_rows)
        .saturating_sub(scroll_offset)
        + visible_row;
    (col, stable_row)
}

/// Find word boundaries around a column in a line's text.
/// Returns (start_col, end_col) inclusive.
pub(crate) fn word_bounds_in_line(line_text: &str, col: usize) -> (usize, usize) {
    let chars: Vec<char> = line_text.chars().collect();
    if chars.is_empty() || col >= chars.len() {
        return (col, col);
    }

    let is_delim = |ch: char| WORD_DELIMITERS.contains(ch);
    let at_delim = is_delim(chars[col]);

    // Walk left
    let mut start = col;
    while start > 0 && is_delim(chars[start - 1]) == at_delim {
        start -= 1;
    }

    // Walk right
    let mut end = col;
    while end + 1 < chars.len() && is_delim(chars[end + 1]) == at_delim {
        end += 1;
    }

    (start, end)
}

/// Extract text from the terminal screen within a selection range.
pub(crate) fn extract_selection_text(
    pane: &TerminalPane,
    start: (usize, usize),
    end: (usize, usize),
    cols: usize,
) -> String {
    let screen = pane.screen();
    let lines = screen.lines_in_phys_range(start.1..end.1 + 1);
    let mut result = String::new();

    for (i, line) in lines.iter().enumerate() {
        let row = start.1 + i;
        let mut line_text = String::new();
        for cell in line.visible_cells() {
            let ci = cell.cell_index();
            if ci >= cols {
                break;
            }
            // Determine if this cell is in the selection
            let in_sel = if start.1 == end.1 {
                ci >= start.0 && ci <= end.0
            } else if row == start.1 {
                ci >= start.0
            } else if row == end.1 {
                ci <= end.0
            } else {
                true
            };
            if in_sel {
                line_text.push_str(cell.str());
            }
        }

        if i > 0 {
            // Check if previous line was wrapped — if so, don't add newline
            if i > 0 {
                let prev_line = &lines[i - 1];
                if !prev_line.last_cell_was_wrapped() {
                    result.push('\n');
                }
            }
        }
        result.push_str(line_text.trim_end());
    }

    result
}

/// Build a flat string of a line's cell text for word boundary detection.
pub(crate) fn line_text_string(pane: &TerminalPane, stable_row: usize, cols: usize) -> String {
    let screen = pane.screen();
    let lines = screen.lines_in_phys_range(stable_row..stable_row + 1);
    if lines.is_empty() {
        return String::new();
    }
    let line = &lines[0];
    let mut text = String::new();
    for cell in line.visible_cells() {
        if cell.cell_index() >= cols {
            break;
        }
        let s = cell.str();
        if s.is_empty() {
            text.push(' ');
        } else {
            text.push_str(s);
        }
    }
    text
}
