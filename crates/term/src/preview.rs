//! Port of terminalPreview.ts.
//!
//! The TS class chained every write/resize on a promise because xterm parses asynchronously; [`HeadlessTerminal`]
//! parses synchronously, so these methods are plain `&mut self` calls (no await, nothing to hold across one) and a
//! read always sees every earlier write. Sizes and scroll offsets stay JS numbers (`f64`) so NaN/±Infinity,
//! fractions and negatives normalise exactly as in TS (the H0c goldens exercise 2.5, -3 and 1e6).

use serde::{Deserialize, Serialize};

use crate::alacritty::AlacrittyTerminal;
use crate::headless::{HeadlessTerminal, TerminalOptions};

/// TS: terminalPreview.ts DEFAULT_SCROLLBACK
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// TS: terminalPreview.ts clampSize — `fallback` when not finite, else `max(1, floor(value))`.
pub fn clamp_size(value: f64, fallback: u16) -> u16 {
    if !value.is_finite() {
        return fallback;
    }
    // `as` saturates: sizes beyond u16 are not representable by the terminal anyway.
    value.floor().max(1.0) as u16
}

/// TS: terminalPreview.ts stripTrailingEmptyLines
pub fn strip_trailing_empty_lines(lines: Vec<String>) -> Vec<String> {
    let mut output = lines;
    while output.last().is_some_and(String::is_empty) {
        output.pop();
    }
    output
}

/// TS: terminalPreview.ts TerminalPreview.getScrollInfo result `{scrollOffset, maxScrollOffset}`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollInfo {
    pub scroll_offset: usize,
    pub max_scroll_offset: usize,
}

/// TS: terminalPreview.ts TerminalPreview
pub struct TerminalPreview<T: HeadlessTerminal = AlacrittyTerminal> {
    terminal: T,
    snapshot: String,
    snapshot_scroll_offset: usize,
    dirty: bool,
    cols: u16,
    rows: u16,
}

impl<T: HeadlessTerminal> TerminalPreview<T> {
    /// TS: new TerminalPreview(cols, rows) — sizes clamped with fallbacks 80×24, scrollback [`DEFAULT_SCROLLBACK`].
    pub fn new(cols: f64, rows: f64) -> Self {
        let cols = clamp_size(cols, 80);
        let rows = clamp_size(rows, 24);
        TerminalPreview {
            terminal: T::new(TerminalOptions {
                cols,
                rows,
                scrollback: DEFAULT_SCROLLBACK,
            }),
            snapshot: String::new(),
            snapshot_scroll_offset: 0,
            dirty: true,
            cols,
            rows,
        }
    }

    /// TS: TerminalPreview.write(data) — feeds decoded PTY output and marks the snapshot dirty.
    pub fn write(&mut self, data: &str) {
        self.terminal.write_str(data);
        self.dirty = true;
    }

    /// TS: TerminalPreview.resize(cols, rows) — no-op when the clamped size is unchanged.
    pub fn resize(&mut self, cols: f64, rows: f64) {
        let next_cols = clamp_size(cols, self.cols);
        let next_rows = clamp_size(rows, self.rows);
        if next_cols == self.cols && next_rows == self.rows {
            return;
        }
        self.cols = next_cols;
        self.rows = next_rows;
        self.terminal.resize(self.cols, self.rows);
        self.dirty = true;
    }

    /// TS: TerminalPreview.getSnapshot(scrollOffset = 0) — viewport text at a scroll offset, cached.
    pub fn get_snapshot(&mut self, scroll_offset: Option<f64>) -> String {
        let normalized = self.normalize_scroll_offset(scroll_offset.unwrap_or(0.0));
        if self.dirty || normalized != self.snapshot_scroll_offset {
            self.refresh_snapshot(normalized as f64);
            self.dirty = false;
        }
        self.snapshot.clone()
    }

    /// TS: TerminalPreview.getScrollInfo(scrollOffset = 0)
    pub fn get_scroll_info(&self, scroll_offset: Option<f64>) -> ScrollInfo {
        let max_scroll_offset = self.get_max_scroll_offset();
        ScrollInfo {
            scroll_offset: self.normalize_scroll_offset(scroll_offset.unwrap_or(0.0)),
            max_scroll_offset,
        }
    }

    /// TS: TerminalPreview.getAnsiFrame — clear + per-row cursor moves + text + final cursor position.
    pub fn get_ansi_frame(&mut self) -> String {
        if self.dirty {
            self.refresh_snapshot(0.0);
            self.dirty = false;
        }
        let start_line = self.terminal.base_y();
        let mut frame = String::from("\x1b[2J\x1b[H");
        for index in 0..usize::from(self.rows) {
            let text = self.terminal.line_text(start_line + index, true).unwrap_or_default();
            if !text.is_empty() {
                frame.push_str(&format!("\x1b[{};1H{text}", index + 1));
            }
        }
        let cursor = self.terminal.cursor();
        frame.push_str(&format!(
            "\x1b[{};{}H",
            u32::from(cursor.y) + 1,
            u32::from(cursor.x) + 1
        ));
        frame
    }

    /// TS: TerminalPreview.getCachedSnapshot — the last computed snapshot without refreshing.
    pub fn get_cached_snapshot(&self) -> String {
        self.snapshot.clone()
    }

    /// TS: TerminalPreview.dispose
    pub fn dispose(self) {
        drop(self);
    }

    /// The preview's own terminal (term.gen.ts reads `preview.terminal` for the golden screen snapshot).
    pub fn terminal(&self) -> &T {
        &self.terminal
    }

    /// TS: TerminalPreview.refreshSnapshot(scrollOffset = 0)
    fn refresh_snapshot(&mut self, scroll_offset: f64) {
        let normalized = self.normalize_scroll_offset(scroll_offset);
        let start_line = self.terminal.base_y().saturating_sub(normalized);
        let rendered: Vec<String> = (0..usize::from(self.rows))
            .map(|index| self.terminal.line_text(start_line + index, true).unwrap_or_default())
            .collect();
        self.snapshot = strip_trailing_empty_lines(rendered).join("\n");
        self.snapshot_scroll_offset = normalized;
    }

    /// TS: TerminalPreview.getMaxScrollOffset — `max(0, buffer.active.baseY)`.
    fn get_max_scroll_offset(&self) -> usize {
        self.terminal.base_y()
    }

    /// TS: TerminalPreview.normalizeScrollOffset — 0 when not finite, else floor clamped to `[0, max]`.
    fn normalize_scroll_offset(&self, scroll_offset: f64) -> usize {
        if !scroll_offset.is_finite() {
            return 0;
        }
        let max = self.get_max_scroll_offset();
        let floored = scroll_offset.floor().max(0.0);
        if floored >= max as f64 { max } else { floored as usize }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::*;
    use crate::utf8::Utf8Decoder;

    type Preview = TerminalPreview<AlacrittyTerminal>;

    #[test]
    fn clamp_size_matches_js() {
        assert_eq!(clamp_size(f64::NAN, 80), 80);
        assert_eq!(clamp_size(f64::INFINITY, 24), 24);
        assert_eq!(clamp_size(f64::NEG_INFINITY, 7), 7);
        assert_eq!(clamp_size(2.9, 80), 2);
        assert_eq!(clamp_size(0.0, 80), 1);
        assert_eq!(clamp_size(-3.0, 80), 1);
        assert_eq!(clamp_size(1e9, 80), u16::MAX);
    }

    #[test]
    fn strip_trailing_empty_lines_keeps_inner_blanks() {
        let lines = |items: &[&str]| items.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        assert_eq!(
            strip_trailing_empty_lines(lines(&["a", "", "b", "", ""])),
            lines(&["a", "", "b"])
        );
        assert!(strip_trailing_empty_lines(lines(&["", ""])).is_empty());
        assert!(strip_trailing_empty_lines(Vec::new()).is_empty());
    }

    #[test]
    fn constructor_falls_back_to_80x24() {
        let preview = Preview::new(f64::NAN, f64::INFINITY);
        assert_eq!((preview.terminal().cols(), preview.terminal().rows()), (80, 24));
        let preview = Preview::new(1.0, 0.5);
        // xterm keeps at least two columns; the preview's own clamp says one.
        assert_eq!((preview.terminal().cols(), preview.terminal().rows()), (2, 1));
    }

    #[test]
    fn snapshot_is_cached_until_dirty() {
        let mut preview = Preview::new(10.0, 3.0);
        assert_eq!(preview.get_cached_snapshot(), "");
        preview.write("one\r\ntwo");
        assert_eq!(preview.get_cached_snapshot(), "");
        assert_eq!(preview.get_snapshot(None), "one\ntwo");
        preview.write("\r\nthree\r\nfour");
        assert_eq!(preview.get_cached_snapshot(), "one\ntwo");
        assert_eq!(preview.get_snapshot(None), "two\nthree\nfour");
        assert_eq!(preview.get_snapshot(Some(1.0)), "one\ntwo\nthree");
        assert_eq!(preview.get_cached_snapshot(), "one\ntwo\nthree");
        assert_eq!(
            preview.get_scroll_info(Some(7.0)),
            ScrollInfo {
                scroll_offset: 1,
                max_scroll_offset: 1
            }
        );
        assert_eq!(preview.get_scroll_info(Some(f64::NAN)).scroll_offset, 0);
        assert_eq!(preview.get_scroll_info(Some(-3.0)).scroll_offset, 0);
    }

    #[test]
    fn byte_order_marks_never_reach_the_screen() {
        let mut preview = Preview::new(10.0, 2.0);
        preview.write("a\u{FEFF}b");
        assert_eq!(preview.get_snapshot(None), "ab");
        assert_eq!(preview.terminal().cursor().x, 2);
        assert_eq!(preview.get_ansi_frame(), "\x1b[2J\x1b[H\x1b[1;1Hab\x1b[1;3H");
    }

    #[test]
    fn ansi_frame_places_rows_and_cursor() {
        let mut preview = Preview::new(10.0, 3.0);
        preview.write("ab\r\n\r\ncd");
        assert_eq!(preview.get_ansi_frame(), "\x1b[2J\x1b[H\x1b[1;1Hab\x1b[3;1Hcd\x1b[3;3H");
        assert_eq!(preview.get_cached_snapshot(), "ab\n\ncd");
    }

    #[test]
    fn resize_is_a_no_op_for_the_same_clamped_size() {
        let mut preview = Preview::new(10.0, 3.0);
        preview.write("x");
        let _ = preview.get_snapshot(None);
        preview.resize(10.7, f64::NAN);
        assert!(!preview.dirty);
        preview.resize(5.0, 2.0);
        assert!(preview.dirty);
        assert_eq!((preview.terminal().cols(), preview.terminal().rows()), (5, 2));
        preview.dispose();
    }

    // ── H0c goldens ──────────────────────────────────────────────────────────────────────────────────────────────

    /// Golden checkpoints that may differ from xterm, with the reason. Empty: every checkpoint matches.
    const ALLOWLIST: &[(&str, &str, &str)] = &[];

    fn term_fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/term")
    }

    fn runs_json(terminal: &AlacrittyTerminal, y: usize) -> Option<Value> {
        let runs = terminal.line_runs(y);
        (!runs.is_empty()).then(|| serde_json::to_value(runs).unwrap_or(Value::Null))
    }

    /// The `screen` object of parity/gen/term.gen.ts screenSnapshot().
    fn screen(terminal: &AlacrittyTerminal) -> Value {
        let base = terminal.base_y();
        let mut lines = Vec::new();
        let mut wrapped = Vec::new();
        let mut attrs = serde_json::Map::new();
        let mut wide = serde_json::Map::new();
        for row in 0..usize::from(terminal.rows()) {
            let y = base + row;
            let Some(text) = terminal.line_text(y, true) else {
                lines.push(String::new());
                continue;
            };
            lines.push(text);
            if terminal.line_is_wrapped(y) {
                wrapped.push(row);
            }
            if let Some(runs) = runs_json(terminal, y) {
                attrs.insert(row.to_string(), runs);
            }
            let wide_cells = terminal.wide_cells(y);
            if !wide_cells.is_empty() {
                wide.insert(row.to_string(), json!(wide_cells));
            }
        }
        let scrollback: Vec<String> = (0..base)
            .map(|y| terminal.line_text(y, true).unwrap_or_default())
            .collect();
        let scrollback_wrapped: Vec<usize> = (0..base).filter(|y| terminal.line_is_wrapped(*y)).collect();
        let cursor = terminal.cursor();
        json!({
            "cols": terminal.cols(),
            "rows": terminal.rows(),
            "buffer": terminal.buffer_kind(),
            "cursor": {"x": cursor.x, "y": cursor.y},
            "baseY": base,
            "viewportY": terminal.viewport_y(),
            "length": terminal.length(),
            "lines": lines,
            "wrapped": wrapped,
            "attrs": attrs,
            "wide": wide,
            "scrollback": scrollback,
            "scrollbackWrapped": scrollback_wrapped,
            "modes": terminal.modes(),
            "title": terminal.title(),
        })
    }

    /// The first path at which two JSON values differ, with both sides.
    fn first_difference(path: &str, expected: &Value, actual: &Value) -> Option<String> {
        match (expected, actual) {
            (Value::Object(left), Value::Object(right)) => {
                let keys: std::collections::BTreeSet<&String> = left.keys().chain(right.keys()).collect();
                keys.into_iter().find_map(|key| {
                    first_difference(
                        &format!("{path}.{key}"),
                        left.get(key).unwrap_or(&Value::Null),
                        right.get(key).unwrap_or(&Value::Null),
                    )
                })
            }
            (Value::Array(left), Value::Array(right)) if left.len() == right.len() => left
                .iter()
                .zip(right)
                .enumerate()
                .find_map(|(index, (left, right))| first_difference(&format!("{path}[{index}]"), left, right)),
            _ if expected == actual => None,
            _ => Some(format!("{path}: expected {expected}, got {actual}")),
        }
    }

    fn arg(call: &Value) -> Option<f64> {
        call.get("args").and_then(|args| args.get(0)).and_then(Value::as_f64)
    }

    /// Replays one stream as term.gen.ts golden() did; returns the mismatching checkpoints.
    fn replay_golden(path: &Path) -> (usize, Vec<(String, String)>) {
        let golden: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("golden")).expect("json");
        let name = golden["name"].as_str().expect("name").to_string();
        let bytes = std::fs::read(term_fixtures().join("streams").join(format!("{name}.bin"))).expect("stream");
        let mut decoder = Utf8Decoder::new();
        let mut preview: Option<Preview> = None;
        let mut checkpoints = 0;
        let mut failures = Vec::new();
        for op in golden["ops"].as_array().expect("ops") {
            match op["op"].as_str().expect("op") {
                "new" => {
                    preview = Some(Preview::new(
                        op["cols"].as_f64().expect("cols"),
                        op["rows"].as_f64().expect("rows"),
                    ));
                }
                "write" => {
                    let from = op["from"].as_u64().expect("from") as usize;
                    let to = op["to"].as_u64().expect("to") as usize;
                    let text = decoder.write(&bytes[from..to]);
                    assert_eq!(
                        text.is_empty(),
                        op.get("empty").is_some(),
                        "{name}: empty flag at {from}"
                    );
                    if !text.is_empty() {
                        preview.as_mut().expect("preview").write(&text);
                    }
                }
                "eof" => {
                    let tail = decoder.end();
                    assert!(!tail.is_empty(), "{name}: eof without a held sequence");
                    preview.as_mut().expect("preview").write(&tail);
                }
                "resize" => {
                    preview
                        .as_mut()
                        .expect("preview")
                        .resize(op["cols"].as_f64().expect("cols"), op["rows"].as_f64().expect("rows"));
                }
                "checkpoint" => {
                    checkpoints += 1;
                    let label = op["label"].as_str().expect("label").to_string();
                    let preview = preview.as_mut().expect("preview");
                    let mut problems = Vec::new();
                    if let Some(diff) = first_difference("screen", &op["screen"], &screen(preview.terminal())) {
                        problems.push(diff);
                    }
                    for (index, call) in op["calls"].as_array().expect("calls").iter().enumerate() {
                        let result = match call["fn"].as_str().expect("fn") {
                            "getCachedSnapshot" => json!(preview.get_cached_snapshot()),
                            "getAnsiFrame" => json!(preview.get_ansi_frame()),
                            "getSnapshot" => json!(preview.get_snapshot(arg(call))),
                            "getScrollInfo" => json!(preview.get_scroll_info(arg(call))),
                            other => panic!("{name}: unknown call {other}"),
                        };
                        if let Some(diff) = first_difference(&format!("calls[{index}]"), &call["result"], &result) {
                            problems.push(format!("{} {diff}", call["fn"]));
                        }
                    }
                    if !problems.is_empty() {
                        failures.push((label, problems.join("\n    ")));
                    }
                }
                other => panic!("{name}: unknown op {other}"),
            }
        }
        (checkpoints, failures)
    }

    #[test]
    fn xterm_goldens_replay() {
        let mut goldens: Vec<PathBuf> = std::fs::read_dir(term_fixtures())
            .expect("fixtures/term")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.to_string_lossy().ends_with(".golden.json"))
            .collect();
        goldens.sort();
        assert_eq!(goldens.len(), 15, "expected the 15 H0c goldens");
        let mut total = 0;
        let mut failing: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        let mut allowlisted = 0;
        for path in &goldens {
            let stream = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".golden.json"))
                .unwrap_or_default()
                .to_string();
            let (checkpoints, failures) = replay_golden(path);
            total += checkpoints;
            for (label, diff) in failures {
                if ALLOWLIST.iter().any(|(s, l, _)| *s == stream && *l == label) {
                    allowlisted += 1;
                } else {
                    failing.entry(stream.clone()).or_default().push((label, diff));
                }
            }
        }
        eprintln!("replayed {total} golden checkpoints ({allowlisted} allowlisted)");
        assert!(
            allowlisted <= total / 50,
            "{allowlisted} allowlisted of {total} checkpoints exceeds 2%"
        );
        let report: Vec<String> = failing
            .iter()
            .flat_map(|(stream, failures)| {
                failures
                    .iter()
                    .map(move |(label, diff)| format!("{stream} @ {label}:\n    {diff}"))
            })
            .collect();
        assert!(
            report.is_empty(),
            "{} of {total} golden checkpoints differ:\n{}",
            report.len(),
            report.join("\n")
        );
    }
}
