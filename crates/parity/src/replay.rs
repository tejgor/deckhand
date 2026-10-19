//! The replay runner: feeds every case to the port, compares, applies the unit allowlist, writes the result file.

use std::fmt::Display;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::compare::{diff_with, show};
use crate::fixture::{Case, Expect, Fixture};
use crate::manifest::Manifest;

/// What the port produced for one case: the return value (serialized to tagged JSON) or the error text, and the
/// arguments after the call when the case records mutation (`argsAfter`).
#[derive(Debug, Clone)]
pub struct Outcome {
    pub result: Result<Value, String>,
    pub args_after: Option<Vec<Value>>,
}

impl Outcome {
    pub fn ret(value: impl Serialize) -> Self {
        Self::from(Ok::<_, String>(value))
    }
    pub fn throws(message: impl Display) -> Self {
        Self {
            result: Err(message.to_string()),
            args_after: None,
        }
    }
    pub fn with_args_after(mut self, args: Vec<Value>) -> Self {
        self.args_after = Some(args);
        self
    }
}

/// `Ok(T)` serializes `T` (serde_json with preserve_order); `Err(E)` uses `E`'s `Display` text as the error message.
impl<T: Serialize, E: Display> From<Result<T, E>> for Outcome {
    fn from(result: Result<T, E>) -> Self {
        let result = match result {
            Ok(value) => serde_json::to_value(value)
                .map_err(|error| format!("parity: cannot serialize the return value: {error}")),
            Err(error) => Err(error.to_string()),
        };
        Self {
            result,
            args_after: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub index: usize,
    /// ret | throws | args_after | conversion | panic | harness
    pub kind: String,
    pub allowlisted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub diff: String,
    pub args: Value,
    pub expected: Value,
    pub actual: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub unit: String,
    pub fixture: String,
    pub path: String,
    pub cases: usize,
    pub passed: usize,
    pub failed: usize,
    pub allowlisted: usize,
    /// Allowlisted failures permitted: 2% of the cases, rounded down.
    pub allowlist_limit: usize,
    pub ok: bool,
    /// Problems that fail the replay outright (missing fixture, not listed in the manifest, no cases, ...).
    pub problems: Vec<String>,
    /// Allowlisted case indices that did not fail (remove them from the manifest).
    pub stale_allowlist: Vec<usize>,
    pub failures: Vec<Failure>,
}

impl Report {
    pub fn summary(&self) -> String {
        let mut text = format!(
            "parity {} {}: {} ({} cases, {} failed, {} allowlisted, limit {})",
            self.unit,
            self.fixture,
            if self.ok { "ok" } else { "FAILED" },
            self.cases,
            self.failed,
            self.allowlisted,
            self.allowlist_limit
        );
        for problem in &self.problems {
            text.push_str(&format!("\n  problem: {problem}"));
        }
        for failure in self.failures.iter().take(12) {
            let marker = if failure.allowlisted { " (allowlisted)" } else { "" };
            text.push_str(&format!(
                "\n  case {} [{}]{marker}: args {}",
                failure.index,
                failure.kind,
                show(&failure.args)
            ));
            for line in failure.diff.lines() {
                text.push_str(&format!("\n    {line}"));
            }
        }
        if self.failures.len() > 12 {
            text.push_str(&format!(
                "\n  … {} more (see $PARITY_RESULTS_DIR)",
                self.failures.len() - 12
            ));
        }
        if !self.stale_allowlist.is_empty() {
            text.push_str(&format!(
                "\n  stale allowlist entries (now passing): {:?}",
                self.stale_allowlist
            ));
        }
        text
    }

    /// Panics with the summary unless the replay passed.
    pub fn assert_ok(&self) {
        assert!(self.ok, "{}", self.summary());
    }

    /// Writes `<dir>/<unit>/<fixture>.json` (the runner does this for `$PARITY_RESULTS_DIR`).
    pub fn write_results(&self, dir: &Path) -> Result<PathBuf, String> {
        let path = dir.join(&self.unit).join(format!("{}.json", self.fixture));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(path)
    }
}

/// A configured replay of one fixture of one unit.
pub struct Replay {
    unit: String,
    fixture: String,
    manifest: Result<Manifest, String>,
    key_order: bool,
    fixtures_root: Option<PathBuf>,
    results_dir: Option<Option<PathBuf>>,
}

impl Replay {
    /// Uses `parity/units/<unit>.toml` (allowlist, utf16_fields, fixtures list).
    pub fn new(unit: &str, fixture: &str) -> Self {
        Self {
            unit: unit.to_string(),
            fixture: fixture.to_string(),
            manifest: Manifest::load(unit),
            key_order: false,
            fixtures_root: None,
            results_dir: None,
        }
    }

    /// Uses an explicit manifest (its `id` names the unit).
    pub fn with_manifest(manifest: Manifest, fixture: &str) -> Self {
        Self {
            unit: manifest.id.clone(),
            fixture: fixture.to_string(),
            manifest: Ok(manifest),
            key_order: false,
            fixtures_root: None,
            results_dir: None,
        }
    }

    /// Also require object keys in the TS order (persisted / wire objects).
    pub fn key_order(mut self, on: bool) -> Self {
        self.key_order = on;
        self
    }

    /// Reads fixtures from `root` instead of `fixtures_root()`.
    pub fn fixtures_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.fixtures_root = Some(root.into());
        self
    }

    /// Writes the result file under `dir` (`None`: not at all) instead of `$PARITY_RESULTS_DIR`.
    pub fn results_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.results_dir = Some(dir);
        self
    }

    pub(crate) fn fixture_path(&self, suffix: &str) -> PathBuf {
        self.fixtures_root
            .clone()
            .unwrap_or_else(crate::fixtures_root)
            .join(format!("{}{suffix}", self.fixture))
    }

    pub(crate) fn utf16_fields(&self) -> Option<&indexmap::IndexMap<String, String>> {
        self.manifest
            .as_ref()
            .ok()
            .and_then(|manifest| manifest.utf16_fields.get(&self.fixture))
    }

    pub(crate) fn manifest_problems(&self) -> (Option<&Manifest>, Vec<String>) {
        match &self.manifest {
            Err(error) => (None, vec![error.clone()]),
            Ok(manifest) if !manifest.fixtures.contains(&self.fixture) => (
                Some(manifest),
                vec![format!(
                    "fixture {:?} is not listed in the fixtures of unit {}",
                    self.fixture, self.unit
                )],
            ),
            Ok(manifest) => (Some(manifest), Vec::new()),
        }
    }

    fn load(&self) -> (Option<Fixture>, Vec<String>) {
        let (_, mut problems) = self.manifest_problems();
        match Fixture::load_path(&self.fixture, &self.fixture_path(".jsonl"), self.utf16_fields()) {
            Ok(fixture) => (Some(fixture), problems),
            Err(error) => {
                problems.push(error);
                (None, problems)
            }
        }
    }

    /// Replays every case through `port`; panics in `port` count as failures of that case.
    pub fn run<F, O>(&self, mut port: F) -> Report
    where
        F: FnMut(&Case) -> O,
        O: Into<Outcome>,
    {
        let (fixture, problems) = self.load();
        let Some(fixture) = fixture else {
            return self.finish(Path::new(""), 0, Vec::new(), problems);
        };
        let mut failures = Vec::new();
        for case in &fixture.cases {
            let failure = if let Some(error) = &case.conversion_error {
                Some(conversion_failure(case, error))
            } else {
                match catch_unwind(AssertUnwindSafe(|| port(case).into())) {
                    Ok(outcome) => check(case, &outcome, self.key_order),
                    Err(payload) => Some(panic_failure(case, payload.as_ref())),
                }
            };
            failures.extend(failure);
        }
        self.finish(&fixture.path, fixture.cases.len(), failures, problems)
    }

    /// Like `run` for an async port (drive it with any executor, e.g. a current_thread tokio runtime).
    pub async fn run_async<F, Fut, O>(&self, mut port: F) -> Report
    where
        F: FnMut(&Case) -> Fut,
        Fut: Future<Output = O>,
        O: Into<Outcome>,
    {
        let (fixture, problems) = self.load();
        let Some(fixture) = fixture else {
            return self.finish(Path::new(""), 0, Vec::new(), problems);
        };
        let mut failures = Vec::new();
        for case in &fixture.cases {
            let failure = match &case.conversion_error {
                Some(error) => Some(conversion_failure(case, error)),
                None => check(case, &port(case).await.into(), self.key_order),
            };
            failures.extend(failure);
        }
        self.finish(&fixture.path, fixture.cases.len(), failures, problems)
    }

    /// Applies the allowlist, decides ok, writes the results file.
    pub(crate) fn finish(
        &self,
        path: &Path,
        cases: usize,
        mut failures: Vec<Failure>,
        mut problems: Vec<String>,
    ) -> Report {
        let allowed = self
            .manifest
            .as_ref()
            .map(|manifest| manifest.allowed(&self.fixture))
            .unwrap_or_default();
        for failure in &mut failures {
            if let Some(reason) = allowed.get(&failure.index) {
                failure.allowlisted = true;
                failure.reason = Some(reason.clone());
            }
        }
        let allowlisted = failures.iter().filter(|failure| failure.allowlisted).count();
        let failed = failures.len();
        let stale_allowlist = allowed
            .keys()
            .copied()
            .filter(|index| !failures.iter().any(|failure| failure.index == *index))
            .collect();
        if cases == 0 && problems.is_empty() {
            problems.push("fixture has no cases".into());
        }
        let allowlist_limit = cases / 50;
        if allowlisted > allowlist_limit {
            problems.push(format!(
                "{allowlisted} allowlisted failures exceed 2% of {cases} cases ({allowlist_limit})"
            ));
        }
        let ok = problems.is_empty() && failed == allowlisted;
        let relative = path
            .strip_prefix(crate::repo_root())
            .unwrap_or(path)
            .display()
            .to_string();
        let report = Report {
            unit: self.unit.clone(),
            fixture: self.fixture.clone(),
            path: relative,
            cases,
            passed: cases.saturating_sub(failed),
            failed,
            allowlisted,
            allowlist_limit,
            ok,
            problems,
            stale_allowlist,
            failures,
        };
        let results_dir = match &self.results_dir {
            Some(dir) => dir.clone(),
            None => std::env::var_os("PARITY_RESULTS_DIR").map(PathBuf::from),
        };
        if let Some(dir) = results_dir
            && let Err(error) = report.write_results(&dir)
        {
            eprintln!("parity: could not write results: {error}");
        }
        report
    }
}

fn expected_json(case: &Case) -> Value {
    match &case.expect {
        Expect::Return(value) => json!({"ret": value}),
        Expect::Throws(message) => json!({"throws": message}),
    }
}
fn actual_json(outcome: &Outcome) -> Value {
    match &outcome.result {
        Ok(value) => json!({"ret": value}),
        Err(message) => json!({"throws": message}),
    }
}
fn failure(case: &Case, kind: &str, diff: String, actual: Value) -> Failure {
    Failure {
        index: case.index,
        kind: kind.to_string(),
        allowlisted: false,
        reason: None,
        diff,
        args: Value::Array(case.args.clone()),
        expected: expected_json(case),
        actual,
    }
}
fn conversion_failure(case: &Case, error: &str) -> Failure {
    failure(case, "conversion", format!("utf16_fields: {error}"), Value::Null)
}
fn panic_failure(case: &Case, payload: &(dyn std::any::Any + Send)) -> Failure {
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".into());
    failure(
        case,
        "panic",
        format!("port panicked: {message}"),
        json!({"panic": message}),
    )
}

/// Compares one outcome with the recorded case.
pub(crate) fn check(case: &Case, outcome: &Outcome, key_order: bool) -> Option<Failure> {
    let actual = actual_json(outcome);
    let mismatch = match (&case.expect, &outcome.result) {
        (Expect::Return(expected), Ok(value)) => diff_with(expected, value, key_order).map(|diff| ("ret", diff)),
        (Expect::Return(expected), Err(message)) => Some((
            "throws",
            format!("threw {message:?}, expected return {}", show(expected)),
        )),
        (Expect::Throws(expected), Ok(value)) => Some((
            "throws",
            format!("returned {}, expected throw {expected:?}", show(value)),
        )),
        (Expect::Throws(expected), Err(message)) => {
            (expected != message).then(|| ("throws", format!("error text: expected {expected:?}, got {message:?}")))
        }
    };
    if let Some((kind, diff)) = mismatch {
        return Some(failure(case, kind, diff, actual));
    }
    // Mutation: compare against argsAfter, or (when the port reports its arguments) against the unmutated args.
    let expected_after = case.args_after.as_ref().unwrap_or(&case.args);
    match (&case.args_after, &outcome.args_after) {
        (Some(_), None) => Some(failure(
            case,
            "args_after",
            "the TS call mutated its arguments; return Outcome::with_args_after(..)".into(),
            actual,
        )),
        (_, Some(after)) => diff_with(
            &Value::Array(expected_after.clone()),
            &Value::Array(after.clone()),
            key_order,
        )
        .map(|diff| {
            let diff = diff
                .lines()
                .map(|line| line.replacen('$', "argsAfter", 1))
                .collect::<Vec<_>>()
                .join("\n");
            failure(case, "args_after", diff, json!({"argsAfter": after}))
        }),
        (None, None) => None,
    }
}

/// Replays `fixture` of `unit` with its manifest, writes the result file, and panics unless it passed.
pub fn replay<F, O>(unit: &str, fixture: &str, port: F) -> Report
where
    F: FnMut(&Case) -> O,
    O: Into<Outcome>,
{
    let report = Replay::new(unit, fixture).run(port);
    report.assert_ok();
    report
}

/// Async `replay`.
pub async fn replay_async<F, Fut, O>(unit: &str, fixture: &str, port: F) -> Report
where
    F: FnMut(&Case) -> Fut,
    Fut: Future<Output = O>,
    O: Into<Outcome>,
{
    let report = Replay::new(unit, fixture).run_async(port).await;
    report.assert_ok();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(lines: &[&str], manifest: &str) -> (tempfile::TempDir, Replay) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("fixtures/m")).unwrap();
        std::fs::write(dir.path().join("fixtures/m/f.jsonl"), lines.join("\n") + "\n").unwrap();
        let replay = Replay::with_manifest(Manifest::parse(manifest).unwrap(), "m/f")
            .fixtures_root(dir.path().join("fixtures"))
            .results_dir(Some(dir.path().join("results")));
        (dir, replay)
    }

    fn many(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!(r#"{{"args":[{index}],"ret":{}}}"#, index * 2))
            .collect()
    }

    #[test]
    fn passing_replay_writes_a_result_file() {
        let lines = many(3);
        let (dir, replay) = setup(
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
            "id = \"X1\"\nfixtures = [\"m/f\"]",
        );
        let report = replay.run(|case| case.arg_i64(0).map(|value| value * 2));
        assert!(report.ok, "{}", report.summary());
        assert_eq!((report.cases, report.passed), (3, 3));
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("results/X1/m/f.json")).unwrap()).unwrap();
        assert_eq!(written["ok"], json!(true));
        assert_eq!(written["cases"], json!(3));
    }

    #[test]
    fn failures_must_be_allowlisted_and_within_two_percent() {
        let lines = many(50);
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let port = |case: &Case| -> Result<i64, String> {
            let value = case.arg_i64(0)?;
            if value == 7 || value == 9 {
                Ok(-1)
            } else {
                Ok(value * 2)
            }
        };
        let manifest =
            "id = \"X1\"\nfixtures = [\"m/f\"]\nallowlist = [{ fixture = \"m/f\", case = 7, reason = \"known\" }]";
        let (_dir, replay) = setup(&lines, manifest);
        let report = replay.run(port);
        assert!(!report.ok);
        assert_eq!((report.failed, report.allowlisted), (2, 1));
        assert!(
            report
                .summary()
                .contains("case 9 [ret]: args [9]\n    $: expected 18, got -1"),
            "{}",
            report.summary()
        );
        let manifest = "id = \"X1\"\nfixtures = [\"m/f\"]\nallowlist = [{ fixture = \"m/f\", case = 7, reason = \"a\" }, \
                        { fixture = \"m/f\", case = 9, reason = \"b\" }, { fixture = \"m/f\", case = 11, reason = \"c\" }]";
        let (_dir, replay) = setup(&lines, manifest);
        let report = replay.run(port);
        // Two allowlisted failures of 50 cases exceed 2% (1).
        assert!(!report.ok);
        assert!(report.problems[0].contains("exceed 2%"), "{:?}", report.problems);
        assert_eq!(report.stale_allowlist, vec![11]);
    }

    #[test]
    fn exact_error_text_mutation_and_panics() {
        let lines = [
            r#"{"args":[1],"throws":"width must be positive: 1"}"#,
            r#"{"args":[[1]],"argsAfter":[[1,2]],"ret":{"$u":1}}"#,
            r#"{"args":[3],"ret":3}"#,
            r#"{"args":[[1]],"argsAfter":[[1,2]],"ret":{"$u":1}}"#,
        ];
        let (_dir, replay) = setup(&lines, "id = \"X1\"\nfixtures = [\"m/f\"]");
        let report = replay.run(|case| match case.index {
            0 => Outcome::throws("width must be positive:1"),
            1 => Outcome::ret(crate::tags::undefined()).with_args_after(vec![json!([1, 3])]),
            2 => panic!("boom"),
            _ => Outcome::ret(crate::tags::undefined()),
        });
        let kinds: Vec<(&str, &str)> = report
            .failures
            .iter()
            .map(|failure| (failure.kind.as_str(), failure.diff.as_str()))
            .collect();
        assert_eq!(
            kinds[0],
            (
                "throws",
                "error text: expected \"width must be positive: 1\", got \"width must be positive:1\""
            )
        );
        assert_eq!(kinds[1], ("args_after", "argsAfter[0][1]: expected 2, got 3"));
        assert_eq!(kinds[2], ("panic", "port panicked: boom"));
        assert!(kinds[3].1.contains("with_args_after"), "{:?}", kinds[3]);
    }

    #[test]
    fn unlisted_or_missing_fixtures_fail() {
        let (_dir, replay) = setup(&[r#"{"args":[],"ret":1}"#], "id = \"X1\"");
        let report = replay.run(|_| Ok::<_, String>(1));
        assert!(!report.ok);
        assert!(report.problems[0].contains("not listed"), "{:?}", report.problems);
        let report = Replay::with_manifest(
            Manifest::parse("id = \"X1\"\nfixtures = [\"m/none\"]").unwrap(),
            "m/none",
        )
        .fixtures_root("/nonexistent")
        .results_dir(None)
        .run(|_| Ok::<_, String>(1));
        assert!(!report.ok && report.cases == 0);
    }
}
