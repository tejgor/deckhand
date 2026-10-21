//! Text snapshots: a test renders something (a screen, a pane) to text and compares it with a file in the repository.
//! With `DECKHAND_BLESS` set a difference (or a missing file) rewrites the file instead of failing.

use std::path::Path;

const MAX_ROWS: usize = 8;

/// Compares `actual` (rows joined with `\n`) with the file at `path` (relative to the repository root: the rows, each
/// ending in a newline) row by row, trailing whitespace ignored. Panics with the differing rows unless they match or
/// `crate::blessing()` rewrote the file.
pub fn assert_snapshot(path: impl AsRef<Path>, actual: &str) {
    let relative = path.as_ref();
    let path = crate::repo_root().join(relative);
    let expected = std::fs::read_to_string(&path);
    let differences = match &expected {
        Ok(expected) => differences(expected, actual),
        Err(error) => vec![format!("cannot read it: {error}")],
    };
    if differences.is_empty() {
        return;
    }
    if crate::blessing() {
        let text: String = actual.split('\n').map(|row| format!("{}\n", row.trim_end())).collect();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|error| panic!("{}: {error}", parent.display()));
        }
        std::fs::write(&path, text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        eprintln!("snapshot: blessed {}", relative.display());
        return;
    }
    panic!(
        "snapshot {} differs:\n  {}\n  an intended change? rerun with DECKHAND_BLESS=1 and review the diff",
        relative.display(),
        differences.join("\n  ")
    );
}

/// `expected` is the file (each row ends in a newline), `actual` the rows joined with `\n`.
fn differences(expected: &str, actual: &str) -> Vec<String> {
    let expected: Vec<&str> = expected
        .strip_suffix('\n')
        .unwrap_or(expected)
        .split('\n')
        .map(str::trim_end)
        .collect();
    let actual: Vec<&str> = actual.split('\n').map(str::trim_end).collect();
    let mut out: Vec<String> = (0..expected.len().max(actual.len()))
        .filter_map(|row| {
            let (want, got) = (expected.get(row), actual.get(row));
            (want != got).then(|| {
                format!(
                    "row {row}: expected {}, got {}",
                    want.map_or("<none>".into(), |text| format!("{text:?}")),
                    got.map_or("<none>".into(), |text| format!("{text:?}"))
                )
            })
        })
        .take(MAX_ROWS + 1)
        .collect();
    if out.len() > MAX_ROWS {
        out.truncate(MAX_ROWS);
        out.push("…".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differences_name_rows_and_ignore_trailing_space() {
        assert!(differences("a  \nb\n", "a\nb").is_empty());
        assert!(differences("a\n\n", "a\n").is_empty());
        assert!(differences("a\n", "a").is_empty());
        assert_eq!(
            differences("a\n\n", "a"),
            vec!["row 1: expected \"\", got <none>".to_owned()]
        );
        assert_eq!(
            differences("a\nb\n", "a\nc\nd"),
            vec![
                "row 1: expected \"b\", got \"c\"".to_owned(),
                "row 2: expected <none>, got \"d\"".to_owned()
            ]
        );
    }
}
