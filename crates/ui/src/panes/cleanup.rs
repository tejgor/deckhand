//! What deleting a worktree would lose, as the deletion confirmations show it (x on a session, x in W, and their typed
//! DELETE screens): every reason on a line of its own, then the files themselves. The file marks, legend and lost-file
//! count live in deckhand_core::detail_texts (the DELETE text uses them).

use deckhand_core::detail_texts::{CLEANUP_FILE_LEGEND, cleanup_file_mark, lost_file_count};
use deckhand_core::jsstr::{js_len, js_slice};
use deckhand_protocol::data::{CleanupFile, CleanupInspection};

use crate::primitives::{Row, Seg, TextStyle, Wrap, fit_row};
use crate::theme::THEME;

/// TS: cleanupView.tsx fitPath with a JS number width (the fixtures include negative ones).
fn fit_path_at(file: &str, width: i64) -> String {
    let length = i64::try_from(js_len(file)).unwrap_or(i64::MAX);
    if length <= width {
        return file.to_owned();
    }
    format!("…{}", js_slice(file, length - (width - 1).max(1), None))
}

/// TS: cleanupView.tsx fitPath: a path cut from the front (the file's name is the end of it).
pub fn fit_path(file: &str, width: usize) -> String {
    fit_path_at(file, i64::try_from(width).unwrap_or(i64::MAX))
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
    let files = inspection.files.as_deref().unwrap_or_default();
    let total = lost_file_count(inspection);
    let room = if total > i64::try_from(rows).unwrap_or(i64::MAX) {
        rows.saturating_sub(1)
    } else {
        rows
    };
    let shown = &files[..room.min(files.len())];
    let more = total - i64::try_from(shown.len()).unwrap_or(i64::MAX);
    LostFileRows {
        files: shown.to_vec(),
        more: usize::try_from(more.max(0)).unwrap_or(0),
    }
}

/// One line of the loss list before it is fitted to a width: Ink's elements (`key`, `wrap`, text children).
#[derive(Clone, Debug, PartialEq, Eq)]
enum LossLine {
    /// `· reason` (wrapped).
    Reason(usize, String),
    /// The legend above the files.
    Legend,
    /// A file: its mark (`M`, `?`, `!`) and its path as fitted.
    File(usize, &'static str, String),
    /// `+N more`.
    More(usize),
}

impl LossLine {
    /// The line's segments (the Ink `<Text>`'s children) and how it fits its width.
    fn row(&self) -> (Row, Wrap) {
        match self {
            LossLine::Reason(_, reason) => (
                vec![Seg::new(format!("· {reason}"), TextStyle::fg(THEME.warn))],
                Wrap::Wrap,
            ),
            LossLine::Legend => (
                vec![Seg::new(
                    format!("  Files ({CLEANUP_FILE_LEGEND}):"),
                    TextStyle::fg(THEME.muted),
                )],
                Wrap::TruncateEnd,
            ),
            LossLine::File(_, mark, path) => (
                vec![
                    Seg::new(format!("    {mark} "), TextStyle::fg(THEME.warn)),
                    Seg::plain(path.clone()),
                ],
                Wrap::TruncateEnd,
            ),
            LossLine::More(more) => (
                vec![Seg::new(format!("    +{more} more"), TextStyle::fg(THEME.muted))],
                Wrap::TruncateEnd,
            ),
        }
    }
}

/// cleanupLossLines before fitting: the reasons, the legend, the files (paths cut to `width`) and the `+N more` row.
fn loss_lines(inspection: &CleanupInspection, width: usize, file_rows: usize) -> Vec<LossLine> {
    let LostFileRows { files, more } = lost_file_rows(inspection, file_rows.max(1));
    let path_width = i64::try_from(width.saturating_sub(6).max(4)).unwrap_or(i64::MAX);
    let mut lines: Vec<LossLine> = inspection
        .reasons
        .iter()
        .enumerate()
        .map(|(index, reason)| LossLine::Reason(index, reason.clone()))
        .collect();
    if !files.is_empty() {
        lines.push(LossLine::Legend);
    }
    for (index, file) in files.iter().enumerate() {
        lines.push(LossLine::File(
            index,
            cleanup_file_mark(file.state),
            fit_path_at(&file.path, path_width),
        ));
    }
    if more > 0 && !files.is_empty() {
        lines.push(LossLine::More(more));
    }
    lines
}

/// TS: cleanupView.tsx cleanupLossLines: the reasons (wrapped at `width`, never cut) and the files in at most
/// `file_rows` rows, with a legend above them.
pub fn cleanup_loss_lines(inspection: &CleanupInspection, width: usize, file_rows: usize) -> Vec<Row> {
    loss_lines(inspection, width, file_rows)
        .iter()
        .flat_map(|line| {
            let (row, wrap) = line.row();
            fit_row(&row, width, wrap)
        })
        .collect()
}

/// TS: cleanupView.tsx fileRowsLeft with JS numbers (the fixtures include negative ones).
fn file_rows_left_at(height: i64, used: i64) -> i64 {
    3.max(height.saturating_sub(used))
}

/// TS: cleanupView.tsx fileRowsLeft: rows a confirmation can give the file list (at least three).
pub fn file_rows_left(height: usize, used: usize) -> usize {
    usize::try_from(file_rows_left_at(
        i64::try_from(height).unwrap_or(i64::MAX),
        i64::try_from(used).unwrap_or(i64::MAX),
    ))
    .unwrap_or(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::row_text;
    use deckhand_core::detail_texts::cleanup_override_text;
    use deckhand_protocol::data::CleanupFileState;
    use deckhand_testkit::{Case, replay};
    use serde_json::{Value, json};

    fn inspection(files: usize, counted: usize) -> CleanupInspection {
        CleanupInspection {
            safe: false,
            reasons: vec![format!("{counted} untracked file(s)")],
            dirty_files: 0,
            untracked_files: counted as i64,
            ignored_files: 0,
            files: Some(
                (0..files)
                    .map(|index| CleanupFile {
                        path: format!("notes/{index}.md"),
                        state: CleanupFileState::Untracked,
                    })
                    .collect(),
            ),
            unpublished_commits: None,
            unmerged_commits: None,
        }
    }

    #[test]
    fn deletion_confirmations_list_the_files_that_would_be_lost_cut_to_the_rows_they_have() {
        // All fit: every file, nothing more.
        assert_eq!(
            lost_file_rows(&inspection(3, 3), 5),
            LostFileRows {
                files: inspection(3, 3).files.unwrap(),
                more: 0
            }
        );
        // Too many: the last row says how many more (the counts are complete even past the listed ones).
        let ten = lost_file_rows(&inspection(10, 10), 4);
        let paths: Vec<&str> = ten.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, ["notes/0.md", "notes/1.md", "notes/2.md"]);
        assert_eq!(ten.more, 7);
        assert_eq!(lost_file_rows(&inspection(100, 250), 4).more, 247);
        assert_eq!(lost_file_count(&inspection(100, 250)), 250);
        // Paths are cut from the front, so the file's name stays.
        assert_eq!(
            fit_path("src/components/deeply/nested/Button.tsx", 20),
            "…y/nested/Button.tsx"
        );
        assert_eq!(fit_path("a.txt", 20), "a.txt");
        // The typed-DELETE screen of x on a session lists every one (that pane scrolls).
        let mut counted = inspection(2, 3);
        counted.files = Some(vec![
            CleanupFile {
                path: "x.ts".to_owned(),
                state: CleanupFileState::Changed,
            },
            CleanupFile {
                path: "secrets".to_owned(),
                state: CleanupFileState::Ignored,
            },
        ]);
        let text = cleanup_override_text(Some(&counted));
        assert!(
            text.contains(
                "3 untracked file(s)\n\nFiles (M changed · ? untracked · ! ignored):\n  M x.ts\n  ! secrets\n  +1 more\n\nMain/current"
            ),
            "{text:?}"
        );
        assert!(cleanup_override_text(None).contains("Safety could not be verified"));
    }

    fn inspection_arg(case: &Case) -> Result<CleanupInspection, String> {
        // lostFileCount takes a Pick<CleanupInspection, ...>: the fields it does not read may be missing.
        let mut value: Value = case.arg_as(0)?;
        if let Some(object) = value.as_object_mut() {
            object.entry("safe").or_insert(json!(false));
            object.entry("reasons").or_insert(json!([]));
        }
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn count_arg(case: &Case, index: usize) -> Result<usize, String> {
        // A negative count of rows or columns behaves as zero in every use.
        Ok(usize::try_from(case.arg_i64(index)?.max(0)).unwrap_or(0))
    }

    #[test]
    fn lost_file_count_parity() {
        replay("cleanupView/lostFileCount", |case: &Case| -> Result<i64, String> {
            Ok(lost_file_count(&inspection_arg(case)?))
        });
    }

    #[test]
    fn fit_path_parity() {
        replay("cleanupView/fitPath", |case: &Case| -> Result<String, String> {
            Ok(fit_path_at(case.arg_str(0)?, case.arg_i64(1)?))
        });
    }

    #[test]
    fn lost_file_rows_parity() {
        replay("cleanupView/lostFileRows", |case: &Case| -> Result<Value, String> {
            let result = lost_file_rows(&inspection_arg(case)?, count_arg(case, 1)?);
            Ok(json!({"files": result.files, "more": result.more}))
        });
    }

    #[test]
    fn file_rows_left_parity() {
        replay("cleanupView/fileRowsLeft", |case: &Case| -> Result<i64, String> {
            Ok(file_rows_left_at(case.arg_i64(0)?, case.arg_i64(1)?))
        });
    }

    #[test]
    fn cleanup_loss_lines_parity() {
        replay("cleanupView/cleanupLossLines", |case: &Case| -> Result<Value, String> {
            let lines = loss_lines(&inspection_arg(case)?, count_arg(case, 1)?, count_arg(case, 2)?);
            // The recorded elements: key, color, wrap and the text children (a nested Text for a file's mark).
            Ok(Value::Array(
                lines
                    .iter()
                    .map(|line| match line {
                        LossLine::Reason(index, reason) => json!({
                            "key": format!("reason-{index}"), "color": "yellow", "wrap": "wrap",
                            "children": format!("· {reason}"),
                        }),
                        LossLine::Legend => json!({
                            "key": "legend", "color": "gray", "wrap": "truncate-end",
                            "children": format!("  Files ({CLEANUP_FILE_LEGEND}):"),
                        }),
                        LossLine::File(index, mark, path) => json!({
                            "key": format!("file-{index}"), "wrap": "truncate-end",
                            "children": [{"key": null, "color": "yellow", "children": format!("    {mark} ")}, path],
                        }),
                        LossLine::More(more) => json!({
                            "key": "more", "color": "gray", "children": format!("    +{more} more"),
                        }),
                    })
                    .collect(),
            ))
        });
    }

    #[test]
    fn loss_lines_are_wrapped_reasons_then_a_legend_and_cut_files() {
        let mut found = inspection(5, 9);
        found.reasons = vec!["a reason long enough to need wrapping".to_owned()];
        let rows: Vec<String> = cleanup_loss_lines(&found, 20, 3)
            .iter()
            .map(|row| row_text(row))
            .collect();
        assert_eq!(
            rows,
            [
                // wrap-ansi keeps the space a row breaks at (trim: false).
                "· a reason long ",
                "enough to need ",
                "wrapping",
                "  Files (M changed …",
                "    ? notes/0.md",
                "    ? notes/1.md",
                "    +7 more",
            ]
        );
        assert_eq!(file_rows_left(24, 30), 3);
        assert_eq!(file_rows_left(24, 10), 14);
    }
}
