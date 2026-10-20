//! Directory cases for file-based modules: `fixtures/parity/<module>/<case>/` holds `before/` and `after/` (file
//! bytes), `tree.json` (`{"before":[entries],"after":[entries]}` with dirs, modes and symlinks, which Git does not
//! keep) and `op.json` (`{"fn", "args", "ret"|"throws"}`). Absolute paths under the temp root are written as `$ROOT`.
//!
//! The replay materializes `before` in a fresh temp dir (umask 022), gives the port the op with `$ROOT` replaced by
//! that dir, then compares the outcome and the resulting tree (every file byte-exact, modes, symlink targets). With
//! `DECKHAND_BLESS` set a difference rewrites the case's `ret`/`throws`, `after/` and `tree.json` instead.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fixture::Case;
use crate::replay::{Failure, Outcome, Replay, Report, blessed_case, check, panic_text};

pub const ROOT_PLACEHOLDER: &str = "$ROOT";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    /// dir | file | symlink
    #[serde(rename = "type")]
    pub kind: String,
    /// Octal permission bits, e.g. "0644".
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
    pub files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Deserialize)]
struct TreeFile {
    before: Vec<TreeEntry>,
    after: Vec<TreeEntry>,
}

/// The operation of a directory case; derefs to its `Case` (args with `$ROOT` replaced by the temp root).
#[derive(Debug, Clone)]
pub struct DirOp {
    pub fn_name: String,
    pub case: Case,
}

impl Deref for DirOp {
    type Target = Case;
    fn deref(&self) -> &Case {
        &self.case
    }
}

fn replace_bytes(data: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    while at < data.len() {
        if data[at..].starts_with(needle) {
            out.extend_from_slice(replacement);
            at += needle.len();
        } else {
            out.push(data[at]);
            at += 1;
        }
    }
    out
}

fn map_strings(value: &Value, map: &dyn Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(map(text)),
        Value::Array(items) => Value::Array(items.iter().map(|item| map_strings(item, map)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, item)| (key.clone(), map_strings(item, map)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |error| format!("{}: {error}", path.display())
}

/// Listing of `root` (root excluded) with `root`'s path in contents/targets replaced by `$ROOT`.
pub fn serialize_tree(root: &Path) -> Result<Tree, String> {
    let mut tree = Tree::default();
    let root_text = root.to_string_lossy().into_owned();
    fn walk(directory: &Path, prefix: &str, root: &str, tree: &mut Tree) -> Result<(), String> {
        let mut names: Vec<_> = std::fs::read_dir(directory)
            .map_err(io(directory))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()
            .map_err(io(directory))?;
        names.sort();
        for name in names {
            let full = directory.join(&name);
            let name = name.to_string_lossy();
            let relative = if prefix.is_empty() {
                name.into_owned()
            } else {
                format!("{prefix}/{name}")
            };
            let metadata = std::fs::symlink_metadata(&full).map_err(io(&full))?;
            let mode = format!("{:04o}", metadata.permissions().mode() & 0o7777);
            if metadata.file_type().is_symlink() {
                let target = std::fs::read_link(&full)
                    .map_err(io(&full))?
                    .to_string_lossy()
                    .replace(root, ROOT_PLACEHOLDER);
                tree.entries.push(TreeEntry {
                    path: relative,
                    kind: "symlink".into(),
                    mode,
                    target: Some(target),
                });
            } else if metadata.is_dir() {
                tree.entries.push(TreeEntry {
                    path: relative.clone(),
                    kind: "dir".into(),
                    mode,
                    target: None,
                });
                walk(&full, &relative, root, tree)?;
            } else {
                let data = std::fs::read(&full).map_err(io(&full))?;
                tree.files.insert(
                    relative.clone(),
                    replace_bytes(&data, root.as_bytes(), ROOT_PLACEHOLDER.as_bytes()),
                );
                tree.entries.push(TreeEntry {
                    path: relative,
                    kind: "file".into(),
                    mode,
                    target: None,
                });
            }
        }
        Ok(())
    }
    walk(root, "", &root_text, &mut tree)?;
    tree.entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(tree)
}

fn parse_mode(mode: &str) -> Result<u32, String> {
    u32::from_str_radix(mode, 8).map_err(|_| format!("bad mode {mode:?}"))
}

/// Recreates a recorded tree under `root`: file bytes from `files_dir`, then symlinks, then modes (dirs last,
/// deepest first, so read-only dirs do not block the writes).
pub fn materialize(entries: &[TreeEntry], files_dir: &Path, root: &Path) -> Result<(), String> {
    let root_text = root.to_string_lossy().into_owned();
    for entry in entries {
        let target = root.join(&entry.path);
        match entry.kind.as_str() {
            "dir" => std::fs::create_dir_all(&target).map_err(io(&target))?,
            "file" => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(io(parent))?;
                }
                let source = files_dir.join(&entry.path);
                let data = std::fs::read(&source).map_err(io(&source))?;
                std::fs::write(
                    &target,
                    replace_bytes(&data, ROOT_PLACEHOLDER.as_bytes(), root_text.as_bytes()),
                )
                .map_err(io(&target))?;
            }
            "symlink" => {
                let link = entry
                    .target
                    .as_deref()
                    .unwrap_or_default()
                    .replace(ROOT_PLACEHOLDER, &root_text);
                std::os::unix::fs::symlink(link, &target).map_err(io(&target))?;
            }
            other => return Err(format!("{}: unknown entry type {other:?}", entry.path)),
        }
    }
    let mut ordered: Vec<&TreeEntry> = entries.iter().filter(|entry| entry.kind != "symlink").collect();
    ordered.sort_by_key(|entry| (entry.kind == "dir", std::cmp::Reverse(entry.path.matches('/').count())));
    for entry in ordered {
        let target = root.join(&entry.path);
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(parse_mode(&entry.mode)?))
            .map_err(io(&target))?;
    }
    Ok(())
}

fn first_difference(expected: &[u8], actual: &[u8]) -> String {
    match (std::str::from_utf8(expected), std::str::from_utf8(actual)) {
        (Ok(expected), Ok(actual)) => {
            let (mut left, mut right) = (expected.split('\n'), actual.split('\n'));
            for line in 1.. {
                match (left.next(), right.next()) {
                    (Some(a), Some(b)) if a == b => continue,
                    (a, b) => {
                        return format!(
                            "line {line}: expected {:?}, got {:?}",
                            a.unwrap_or("<eof>"),
                            b.unwrap_or("<eof>")
                        );
                    }
                }
            }
            String::new()
        }
        _ => format!("binary: expected {} bytes, got {} bytes", expected.len(), actual.len()),
    }
}

/// Differences between the recorded tree (`entries` + bytes under `files_dir`) and `actual`.
pub fn diff_tree(entries: &[TreeEntry], files_dir: &Path, actual: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    let actual_entries: BTreeMap<&str, &TreeEntry> = actual
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    for entry in entries {
        let Some(found) = actual_entries.get(entry.path.as_str()) else {
            out.push(format!("tree: missing {} ({})", entry.path, entry.kind));
            continue;
        };
        if found.kind != entry.kind {
            out.push(format!(
                "tree: {} is a {}, expected a {}",
                entry.path, found.kind, entry.kind
            ));
            continue;
        }
        if found.mode != entry.mode {
            out.push(format!(
                "tree: {} mode expected {}, got {}",
                entry.path, entry.mode, found.mode
            ));
        }
        if found.target != entry.target {
            out.push(format!(
                "tree: {} symlink target expected {:?}, got {:?}",
                entry.path, entry.target, found.target
            ));
        }
        if entry.kind == "file" {
            let expected = std::fs::read(files_dir.join(&entry.path)).unwrap_or_default();
            let got = actual.files.get(&entry.path).map(Vec::as_slice).unwrap_or_default();
            if expected != got {
                out.push(format!(
                    "tree: {} content differs: {}",
                    entry.path,
                    first_difference(&expected, got)
                ));
            }
        }
    }
    for entry in &actual.entries {
        if !entries.iter().any(|expected| expected.path == entry.path) {
            out.push(format!("tree: unexpected {} ({})", entry.path, entry.kind));
        }
    }
    out
}

/// Sets the process umask for the lifetime of the guard (TS records with umask 022).
struct Umask(libc::mode_t);
impl Umask {
    fn set(mask: libc::mode_t) -> Self {
        // SAFETY: umask only swaps the process file-mode creation mask; it cannot fail.
        Self(unsafe { libc::umask(mask) })
    }
}
impl Drop for Umask {
    fn drop(&mut self) {
        // SAFETY: as above.
        unsafe { libc::umask(self.0) };
    }
}

impl Replay {
    /// Replays the directory case `fixture` (`<module>/<case>`): `port(root, op)` performs the operation in `root`.
    pub fn run_dir<F, O>(&self, mut port: F) -> Report
    where
        F: FnMut(&Path, &DirOp) -> O,
        O: Into<Outcome>,
    {
        let (_, mut problems) = self.manifest_problems();
        let case_dir = self.fixture_path("");
        match self.run_dir_case(&case_dir, &mut |root, op| port(root, op).into()) {
            Ok(Settled::Blessed) => {
                eprintln!("parity: blessed {}", case_dir.display());
                let mut report = self.finish(&case_dir, 1, Vec::new(), problems);
                report.blessed = 1;
                report
            }
            Ok(Settled::Compared(failure)) => self.finish(
                &case_dir,
                1,
                failure.into_iter().map(|failure| *failure).collect(),
                problems,
            ),
            Err(error) => {
                problems.push(error);
                self.finish(&case_dir, 0, Vec::new(), problems)
            }
        }
    }

    fn run_dir_case(&self, case_dir: &Path, port: &mut dyn FnMut(&Path, &DirOp) -> Outcome) -> Result<Settled, String> {
        let op_path = case_dir.join("op.json");
        let op: Value = serde_json::from_str(&std::fs::read_to_string(&op_path).map_err(io(&op_path))?)
            .map_err(|error| format!("{}: {error}", op_path.display()))?;
        let tree_path = case_dir.join("tree.json");
        let trees: TreeFile = serde_json::from_str(&std::fs::read_to_string(&tree_path).map_err(io(&tree_path))?)
            .map_err(|error| format!("{}: {error}", tree_path.display()))?;
        let fn_name = op.get("fn").and_then(Value::as_str).unwrap_or_default().to_string();

        let _umask = Umask::set(0o022);
        let temp = tempfile::Builder::new()
            .prefix("parity-dir-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let root = temp.path().canonicalize().map_err(io(temp.path()))?;
        let root_text = root.to_string_lossy().into_owned();
        materialize(&trees.before, &case_dir.join("before"), &root)?;

        // The port sees real paths; comparisons happen in placeholder space.
        let recorded = Case::from_value(0, op.clone(), self.utf16_fields())?;
        let substituted = map_strings(&op, &|text| text.replace(ROOT_PLACEHOLDER, &root_text));
        let dir_op = DirOp {
            fn_name,
            case: Case::from_value(0, substituted, self.utf16_fields())?,
        };
        if let Some(error) = &recorded.conversion_error {
            return Err(format!("utf16_fields: {error}"));
        }
        let (outcome, panicked) = match catch_unwind(AssertUnwindSafe(|| port(&root, &dir_op))) {
            Ok(outcome) => (outcome, false),
            Err(payload) => (
                Outcome::throws(format!("port panicked: {}", panic_text(payload.as_ref()))),
                true,
            ),
        };
        let scrub = |text: &str| text.replace(&root_text, ROOT_PLACEHOLDER);
        let outcome = Outcome {
            result: outcome
                .result
                .map(|value| map_strings(&value, &scrub))
                .map_err(|message| scrub(&message)),
            args_after: outcome
                .args_after
                .map(|args| args.iter().map(|value| map_strings(value, &scrub)).collect()),
        };
        let mut failure = check(&recorded, &outcome, false);
        let actual_tree = serialize_tree(&root)?;
        let tree_diff = diff_tree(&trees.after, &case_dir.join("after"), &actual_tree);
        if self.blessing()
            && !panicked
            && !self.allowed().contains_key(&0)
            && (failure.is_some() || !tree_diff.is_empty())
        {
            let op = blessed_case(&recorded, &outcome, self.utf16_fields())?;
            bless_dir_case(case_dir, op, &actual_tree)?;
            return Ok(Settled::Blessed);
        }
        if !tree_diff.is_empty() {
            let diff = tree_diff.join("\n");
            match &mut failure {
                Some(existing) => {
                    existing.diff.push('\n');
                    existing.diff.push_str(&diff);
                }
                None => {
                    failure = Some(Failure {
                        index: 0,
                        kind: "tree".into(),
                        allowlisted: false,
                        reason: None,
                        diff,
                        args: Value::Array(recorded.args.clone()),
                        expected: Value::Null,
                        actual: Value::Null,
                    })
                }
            }
        }
        Ok(Settled::Compared(failure.map(Box::new)))
    }
}

enum Settled {
    Compared(Option<Box<Failure>>),
    Blessed,
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(path, text + "\n").map_err(io(path))
}

/// Rewrites a directory case's expectation: `op.json`'s `ret`/`throws`, `after/` and `tree.json`'s `after`.
fn bless_dir_case(case_dir: &Path, op: Value, actual: &Tree) -> Result<(), String> {
    write_json(&case_dir.join("op.json"), &op)?;
    let tree_path = case_dir.join("tree.json");
    let mut trees: Value = serde_json::from_str(&std::fs::read_to_string(&tree_path).map_err(io(&tree_path))?)
        .map_err(|error| format!("{}: {error}", tree_path.display()))?;
    trees["after"] = serde_json::to_value(&actual.entries).map_err(|error| error.to_string())?;
    write_json(&tree_path, &trees)?;
    let after = case_dir.join("after");
    if after.exists() {
        std::fs::remove_dir_all(&after).map_err(io(&after))?;
    }
    for (path, data) in &actual.files {
        let target = after.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&target, data).map_err(io(&target))?;
    }
    Ok(())
}

/// Replays directory case `fixture` (`<module>/<case>`) of `unit`, writes the result file, panics unless it passed.
pub fn replay_dir<F, O>(unit: &str, fixture: &str, port: F) -> Report
where
    F: FnMut(&Path, &DirOp) -> O,
    O: Into<Outcome>,
{
    let report = Replay::new(unit, fixture).run_dir(port);
    report.assert_ok();
    report
}

/// Every directory case under `<fixtures root>/<module>` (sub-directories holding an op.json), sorted.
pub fn dir_cases(module: &str) -> Vec<String> {
    let base = crate::fixtures_root().join(module);
    let mut out: Vec<String> = std::fs::read_dir(&base)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("op.json").is_file())
                .map(|entry| format!("{module}/{}", entry.file_name().to_string_lossy()))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_and_materialize_round_trip() {
        let source = tempfile::tempdir().unwrap();
        let root = source.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a/empty")).unwrap();
        std::fs::write(
            root.join("a/config.json"),
            format!("{{\"path\":\"{}/x\"}}\n", root.display()),
        )
        .unwrap();
        std::fs::set_permissions(root.join("a/config.json"), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(root.join("a/config.json"), root.join("link")).unwrap();
        let tree = serialize_tree(&root).unwrap();
        assert_eq!(
            tree.entries.iter().map(|entry| entry.path.as_str()).collect::<Vec<_>>(),
            ["a", "a/config.json", "a/empty", "link"]
        );
        assert_eq!(tree.files["a/config.json"], b"{\"path\":\"$ROOT/x\"}\n");
        assert_eq!(tree.entries[1].mode, "0600");
        assert_eq!(tree.entries[3].target.as_deref(), Some("$ROOT/a/config.json"));

        // Write the bytes the way the recorder does, then rebuild elsewhere.
        let files = tempfile::tempdir().unwrap();
        for (path, data) in &tree.files {
            std::fs::create_dir_all(files.path().join(path).parent().unwrap()).unwrap();
            std::fs::write(files.path().join(path), data).unwrap();
        }
        let target = tempfile::tempdir().unwrap();
        let target_root = target.path().canonicalize().unwrap();
        materialize(&tree.entries, files.path(), &target_root).unwrap();
        let rebuilt = serialize_tree(&target_root).unwrap();
        assert_eq!(rebuilt, tree);
        assert!(diff_tree(&tree.entries, files.path(), &rebuilt).is_empty());

        std::fs::write(target_root.join("a/config.json"), "{}\n").unwrap();
        std::fs::write(target_root.join("extra"), "").unwrap();
        let diff = diff_tree(&tree.entries, files.path(), &serialize_tree(&target_root).unwrap()).join("\n");
        assert!(diff.contains("tree: a/config.json content differs: line 1"), "{diff}");
        assert!(diff.contains("tree: unexpected extra (file)"), "{diff}");
    }

    #[test]
    fn blessing_rewrites_the_result_and_the_after_tree() {
        let fixtures = tempfile::tempdir().unwrap();
        let case_dir = fixtures.path().join("m/write");
        std::fs::create_dir_all(case_dir.join("after")).unwrap();
        std::fs::write(case_dir.join("after/old.txt"), "old\n").unwrap();
        std::fs::write(
            case_dir.join("op.json"),
            r#"{"fn": "write", "args": ["$ROOT"], "ret": "old"}"#,
        )
        .unwrap();
        std::fs::write(
            case_dir.join("tree.json"),
            r#"{"before": [], "after": [{"path": "old.txt", "type": "file", "mode": "0644"}]}"#,
        )
        .unwrap();
        let replay = |bless: bool| {
            Replay::with_manifest(
                crate::Manifest::parse("id = \"X1\"\nfixtures = [\"m/write\"]").unwrap(),
                "m/write",
            )
            .fixtures_root(fixtures.path())
            .results_dir(None)
            .bless(bless)
            .run_dir(|root, _op| {
                std::fs::write(root.join("new.txt"), format!("{}\n", root.display())).unwrap();
                Ok::<_, String>("new")
            })
        };
        let failed = replay(false);
        assert!(!failed.ok);
        assert!(failed.summary().contains("DECKHAND_BLESS=1"), "{}", failed.summary());

        let blessed = replay(true);
        assert!(blessed.ok && blessed.blessed == 1, "{}", blessed.summary());
        let op: Value = serde_json::from_str(&std::fs::read_to_string(case_dir.join("op.json")).unwrap()).unwrap();
        assert_eq!(op, serde_json::json!({"fn": "write", "args": ["$ROOT"], "ret": "new"}));
        assert!(!case_dir.join("after/old.txt").exists());
        assert_eq!(
            std::fs::read_to_string(case_dir.join("after/new.txt")).unwrap(),
            "$ROOT\n"
        );
        let after = replay(false);
        assert!(after.ok && after.blessed == 0, "{}", after.summary());
    }
}
