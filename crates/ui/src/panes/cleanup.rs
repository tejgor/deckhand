//! Port of cleanupView.tsx: what deleting a worktree would lose, as the deletion confirmations show it (x on a
//! session, x in W, and their typed DELETE screens): every reason on a line of its own, then the files themselves.
//! The file marks, legend and lost-file count live in deckhand_core::detail_texts (the DELETE text uses them).

use deckhand_protocol::data::{CleanupFile, CleanupInspection};

use crate::primitives::Row;

/// TS: cleanupView.tsx fitPath: a path cut from the front (the file's name is the end of it).
pub fn fit_path(file: &str, width: usize) -> String {
    let _ = (file, width);
    todo!("U3")
}

/// TS: cleanupView.tsx lostFileRows's result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LostFileRows {
    pub files: Vec<CleanupFile>,
    /// How many files the rows do not list (`+N more`).
    pub more: usize,
}

/// TS: cleanupView.tsx lostFileRows: the files in at most `rows` rows (the last says `+N more` when they do not all
/// fit).
pub fn lost_file_rows(inspection: &CleanupInspection, rows: usize) -> LostFileRows {
    let _ = (inspection, rows);
    todo!("U3")
}

/// TS: cleanupView.tsx cleanupLossLines: the reasons (wrapped at `width`, never cut) and the files in at most
/// `file_rows` rows, with a legend above them.
pub fn cleanup_loss_lines(inspection: &CleanupInspection, width: usize, file_rows: usize) -> Vec<Row> {
    let _ = (inspection, width, file_rows);
    todo!("U3")
}

/// TS: cleanupView.tsx fileRowsLeft: rows a confirmation can give the file list (at least three).
pub fn file_rows_left(height: usize, used: usize) -> usize {
    let _ = (height, used);
    todo!("U3")
}
