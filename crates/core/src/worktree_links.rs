//! Worktree links (`worktree.symlink`, `worktree.files`) and location templates: the pure parts (file I/O is in
//! deckhand_sys::worktree_links).
//!
//! Declarative worktree settings (the `worktree` section of global defaults or deckhand.json): where new worktrees go
//! and which heavy directories / private files are symlinked into them. Applying the links to a new worktree (and
//! `userSlug`, which reads the OS user) is I/O and lives in `deckhand_sys::worktree_links`.

use std::collections::HashSet;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::{Captures, Regex};
use serde_json::{Map, Value};

use crate::error::{DhError, Result};
use crate::jsstr::{self, path};
use deckhand_protocol::data::{BranchFrom, TemplateVars, WorktreeSettings};

/// TS: worktreeLinks.ts BRANCH_FROM.
pub const BRANCH_FROM: [BranchFrom; 3] = [BranchFrom::Current, BranchFrom::Default, BranchFrom::Origin];

/// TS: worktreeLinks.ts PLACEHOLDERS (`{name}` and friends in worktree templates).
pub const PLACEHOLDERS: [&str; 5] = ["name", "repo", "repoParent", "repoRoot", "home"];

/// TS: worktreeLinks.ts MAX_ENTRIES (per `symlink` list and `files` map).
pub const MAX_ENTRIES: usize = 50;

/// TS: worktreeLinks.ts MAX_PATH (UTF-16 units).
pub const MAX_PATH: usize = 4096;

/// `/\{([^{}]*)\}/g`: one placeholder.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{([^{}]*)\}").expect("valid regex"));

/// `/[{}]/.test(template.replace(/\{[^{}]*\}/g, ''))`: a brace that is not part of a `{placeholder}`.
fn unbalanced(template: &str) -> bool {
    PLACEHOLDER.replace_all(template, "").contains(['{', '}'])
}

/// `template.replace(/\{([^{}]*)\}/g, ...)` with a replacer that may throw (the first throw wins, left to right).
fn replace_placeholders(template: &str, mut value: impl FnMut(&str) -> Result<String>) -> Result<String> {
    let mut error = None;
    let expanded = PLACEHOLDER.replace_all(template, |captures: &Captures<'_>| {
        if error.is_some() {
            return String::new();
        }
        let key = captures.get(1).map_or("", |key| key.as_str());
        match value(key) {
            Ok(text) => text,
            Err(problem) => {
                error = Some(problem);
                String::new()
            }
        }
    });
    match error {
        Some(error) => Err(error),
        None => Ok(expanded.into_owned()),
    }
}

/// TS: worktreeLinks.ts expandWorktreeTemplate (placeholders and a leading `~/`; unknown placeholders and unbalanced
/// braces are errors; the result is not resolved).
pub fn expand_worktree_template(template: &str, vars: &TemplateVars) -> Result<String> {
    let quoted = || jsstr::json_stringify_str(template);
    if unbalanced(template) {
        return Err(DhError::msg(format!("Unbalanced braces in {}", quoted())));
    }
    let expanded = replace_placeholders(template, |key| {
        Ok(match key {
            "name" => vars.name.clone(),
            "repo" => vars.repo.clone(),
            "repoParent" => vars.repo_parent.clone(),
            "repoRoot" => vars.repo_root.clone(),
            "home" => vars.home.clone(),
            _ => return Err(DhError::msg(format!("Unknown placeholder {{{key}}} in {}", quoted()))),
        })
    })?;
    Ok(if expanded == "~" {
        vars.home.clone()
    } else if let Some(rest) = expanded.strip_prefix("~/") {
        path::join(&[&vars.home, rest])
    } else {
        expanded
    })
}

/// TS: worktreeLinks.ts worktreeLocation (the template must expand to an absolute path; resolved).
pub fn worktree_location(template: &str, vars: &TemplateVars) -> Result<String> {
    let expanded = expand_worktree_template(template, vars)?;
    if !path::is_absolute(&expanded) {
        return Err(DhError::msg(format!(
            "worktree.location must expand to an absolute path: {expanded}"
        )));
    }
    Ok(path::resolve(&expanded))
}

/// TS: worktreeLinks.ts BranchVars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BranchVars<'a> {
    pub name: &'a str,
    pub user: &'a str,
}

/// TS: worktreeLinks.ts expandBranchName (`{name}` and `{user}` only).
pub fn expand_branch_name(template: &str, vars: &BranchVars<'_>) -> Result<String> {
    let quoted = || jsstr::json_stringify_str(template);
    if unbalanced(template) {
        return Err(DhError::msg(format!("Unbalanced braces in {}", quoted())));
    }
    replace_placeholders(template, |key| match key {
        "name" => Ok(vars.name.to_owned()),
        "user" => Ok(vars.user.to_owned()),
        _ => Err(DhError::msg(format!(
            "Unknown placeholder {{{key}}} in branch name {}",
            quoted()
        ))),
    })
}

/// `/^[-/.]|[/.]$|\.lock$|\/\.|\.lock\//`.
static BAD_BRANCH_SHAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-/.]|[/.]$|\.lock$|/\.|\.lock/").expect("valid regex"));

/// TS: worktreeLinks.ts branchNameProblem (a static `git check-ref-format --branch`; `None`: plausible).
pub fn branch_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || jsstr::js_len(name) > 200 {
        return Some("empty or too long");
    }
    if name
        .chars()
        .any(|ch| ch <= '\u{20}' || matches!(ch, '\u{7f}' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
    {
        return Some("contains spaces, control characters or one of ~ ^ : ? * [ \\");
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") || name == "@" {
        return Some("contains \"..\", \"@{\" or \"//\"");
    }
    if BAD_BRANCH_SHAPE.is_match(name) {
        return Some("starts with -, / or ., ends with / or ., or has a component starting with . or ending in .lock");
    }
    None
}

static NOT_SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9._-]+").expect("valid regex"));
static DOT_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.{2,}").expect("valid regex"));

/// The pure half of worktreeLinks.ts userSlug: an OS user name as a branch-safe slug (`user` when nothing is left).
pub fn slugify_user(name: &str) -> String {
    let lower = jsstr::js_to_lower(name);
    let dashed = NOT_SLUG.replace_all(&lower, "-");
    let trimmed = dashed.trim_matches(['-', '.']);
    let slug = DOT_RUN.replace_all(trimmed, ".");
    if slug.is_empty() {
        "user".to_owned()
    } else {
        slug.into_owned()
    }
}

/// TS: worktreeLinks.ts DUMMY (the vars templates are checked with).
pub fn dummy_template_vars() -> TemplateVars {
    TemplateVars {
        name: "name".into(),
        repo: "repo".into(),
        repo_parent: "/parent".into(),
        repo_root: "/parent/repo".into(),
        home: "/home".into(),
    }
}

/// TS: worktreeLinks.ts templateString (a non-empty string of at most MAX_PATH without NUL that expands; errors
/// read `Invalid <label>` / `Invalid <label>: <why>`).
pub fn template_string(value: &Value, label: &str) -> Result<String> {
    let text = match value.as_str() {
        Some(text) if !jsstr::js_trim(text).is_empty() && jsstr::js_len(text) <= MAX_PATH && !text.contains('\0') => {
            text
        }
        _ => return Err(DhError::msg(format!("Invalid {label}"))),
    };
    expand_worktree_template(text, &dummy_template_vars())
        .map_err(|error| DhError::msg(format!("Invalid {label}: {error}")))?;
    Ok(text.to_owned())
}

/// TS: worktreeLinks.ts validRelativePath (normalized, relative, inside its root, not `.git`). Non-strings are never
/// valid: callers pass `value.as_str()`.
pub fn valid_relative_path(value: &str) -> bool {
    if value.is_empty() || jsstr::js_len(value) > MAX_PATH || value.contains(['\0', '\\']) || path::is_absolute(value) {
        return false;
    }
    if path::normalize(value) != value || value.ends_with('/') {
        return false;
    }
    let segments: Vec<&str> = value.split('/').collect();
    !segments.iter().any(|segment| matches!(*segment, ".." | "." | "")) && segments.first() != Some(&".git")
}

/// `JSON.stringify(value)` for the values these messages quote (strings as JS writes them, others compactly).
fn stringify(value: &Value) -> String {
    match value {
        Value::String(text) => jsstr::json_stringify_str(text),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// A canonical array index (`Object.keys` lists these first, ascending).
fn array_index(key: &str) -> Option<u32> {
    let index: u32 = key.parse().ok()?;
    (index != u32::MAX && index.to_string() == key).then_some(index)
}

/// `Object.keys(object)` order: array indices ascending, then the other keys in insertion order.
fn js_keys(object: &Map<String, Value>) -> Vec<&String> {
    let mut indices: Vec<(u32, &String)> = object
        .keys()
        .filter_map(|key| array_index(key).map(|index| (index, key)))
        .collect();
    indices.sort_by_key(|(index, _)| *index);
    let mut keys: Vec<&String> = indices.into_iter().map(|(_, key)| key).collect();
    keys.extend(object.keys().filter(|key| array_index(key).is_none()));
    keys
}

/// Reorders a map into JS own-key order (array indices ascending, then the rest in insertion order), the order a
/// parsed TS object, or an object spread, enumerates its keys in.
fn into_js_order<V>(mut map: IndexMap<String, V>) -> IndexMap<String, V> {
    let rank = |key: &str| array_index(key).map_or((1, 0), |index| (0, index));
    map.sort_by(|left, _, right, _| rank(left).cmp(&rank(right)));
    map
}

/// The keys a `worktree` setting may hold. `hook` switched a repository's worktree creation script on or off: Deckhand
/// no longer runs those, and files that still set it read without complaint (it is ignored).
const SETTING_KEYS: [&str; 6] = ["location", "symlink", "files", "branchFrom", "branchName", "hook"];

/// TS: worktreeLinks.ts validateWorktreeSettings (the `worktree` value of a deckhand.json-shaped object).
pub fn validate_worktree_settings(value: &Value) -> Result<WorktreeSettings> {
    let Some(record) = value.as_object() else {
        return Err(DhError::msg("worktree must be an object"));
    };
    for key in js_keys(record) {
        if !SETTING_KEYS.contains(&key.as_str()) {
            return Err(DhError::msg(format!("Unknown worktree setting: {key}")));
        }
    }
    if let Some(branch_from) = record.get("branchFrom")
        && !BRANCH_FROM
            .iter()
            .any(|candidate| branch_from.as_str() == Some(candidate.as_str()))
    {
        let names: Vec<&str> = BRANCH_FROM.iter().map(|candidate| candidate.as_str()).collect();
        return Err(DhError::msg(format!(
            "worktree.branchFrom must be one of {}",
            names.join(", ")
        )));
    }
    if let Some(branch_name) = record.get("branchName") {
        let Some(template) = branch_name.as_str().filter(|template| template.contains("{name}")) else {
            return Err(DhError::msg("worktree.branchName must be a template containing {name}"));
        };
        let example = expand_branch_name(
            template,
            &BranchVars {
                name: "name",
                user: "user",
            },
        )
        .map_err(|error| DhError::msg(format!("Invalid worktree.branchName: {error}")))?;
        if let Some(problem) = branch_name_problem(&example) {
            return Err(DhError::msg(format!(
                "Invalid worktree.branchName {}: {problem}",
                jsstr::json_stringify_str(template)
            )));
        }
    }
    if let Some(location) = record.get("location") {
        let location = template_string(location, "worktree.location")?;
        if !location.contains("{name}") {
            return Err(DhError::msg("worktree.location must contain {name}"));
        }
        if !path::is_absolute(&expand_worktree_template(&location, &dummy_template_vars())?) {
            return Err(DhError::msg(
                "worktree.location must be absolute (start with /, ~/, {home}, {repoParent} or {repoRoot})",
            ));
        }
    }
    if let Some(symlink) = record.get("symlink") {
        let Some(entries) = symlink.as_array() else {
            return Err(DhError::msg("worktree.symlink must be a list of relative paths"));
        };
        if entries.len() > MAX_ENTRIES {
            return Err(DhError::msg(format!(
                "At most {MAX_ENTRIES} worktree.symlink entries are supported"
            )));
        }
        for entry in entries {
            if !entry.as_str().is_some_and(valid_relative_path) {
                return Err(DhError::msg(format!(
                    "Invalid worktree.symlink path: {}",
                    stringify(entry)
                )));
            }
        }
    }
    if let Some(files) = record.get("files") {
        let Some(files) = files.as_object() else {
            return Err(DhError::msg("worktree.files must map destinations to sources"));
        };
        if files.len() > MAX_ENTRIES {
            return Err(DhError::msg(format!(
                "At most {MAX_ENTRIES} worktree.files entries are supported"
            )));
        }
        for destination in js_keys(files) {
            if !valid_relative_path(destination)
                || ["__proto__", "constructor", "prototype"].contains(&destination.as_str())
            {
                return Err(DhError::msg(format!(
                    "Invalid worktree.files destination: {}",
                    jsstr::json_stringify_str(destination)
                )));
            }
            let source = files.get(destination.as_str()).unwrap_or(&Value::Null);
            template_string(source, &format!("worktree.files source for {destination}"))?;
        }
    }
    let mut settings: WorktreeSettings =
        serde_json::from_value(value.clone()).map_err(|error| DhError::msg(error.to_string()))?;
    settings.files = settings.files.map(into_js_order);
    Ok(settings)
}

/// TS: worktreeLinks.ts mergeWorktreeSettings (repository scalars win; symlinks are a deduplicated union, global
/// first; files merge by destination, repository winning). The TS literal's key order is location, branchFrom,
/// branchName, symlink, files: callers that serialise the result record that order.
pub fn merge_worktree_settings(
    global: Option<&WorktreeSettings>,
    repository: Option<&WorktreeSettings>,
) -> Option<WorktreeSettings> {
    if global.is_none() && repository.is_none() {
        return None;
    }
    // `repository?.[key] ?? global?.[key]`, kept unless undefined or ''.
    fn pick<T: Clone>(
        global: Option<&WorktreeSettings>,
        repository: Option<&WorktreeSettings>,
        get: impl Fn(&WorktreeSettings) -> Option<&T>,
    ) -> Option<T> {
        repository.and_then(&get).or_else(|| global.and_then(&get)).cloned()
    }
    let non_empty = |value: Option<String>| value.filter(|text| !text.is_empty());
    let mut merged = WorktreeSettings {
        location: non_empty(pick(global, repository, |settings| settings.location.as_ref())),
        branch_from: pick(global, repository, |settings| settings.branch_from.as_ref()),
        branch_name: non_empty(pick(global, repository, |settings| settings.branch_name.as_ref())),
        ..WorktreeSettings::default()
    };
    let global_symlink = global.and_then(|settings| settings.symlink.as_ref());
    let repository_symlink = repository.and_then(|settings| settings.symlink.as_ref());
    if global_symlink.is_some() || repository_symlink.is_some() {
        let mut seen = HashSet::new();
        merged.symlink = Some(
            global_symlink
                .into_iter()
                .chain(repository_symlink)
                .flatten()
                .filter(|entry| seen.insert(entry.as_str()))
                .cloned()
                .collect(),
        );
    }
    let global_files = global.and_then(|settings| settings.files.as_ref());
    let repository_files = repository.and_then(|settings| settings.files.as_ref());
    if global_files.is_some() || repository_files.is_some() {
        let mut files = IndexMap::new();
        for (destination, source) in global_files.into_iter().chain(repository_files).flatten() {
            files.insert(destination.clone(), source.clone());
        }
        merged.files = Some(into_js_order(files));
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, Outcome, replay};

    fn vars(case: &Case, index: usize) -> std::result::Result<TemplateVars, String> {
        case.arg_as(index)
    }

    /// Runs a port body whose harness steps may fail (argument decoding).
    fn port(body: impl FnOnce() -> Result<Outcome, String>) -> Outcome {
        body().unwrap_or_else(|error| Outcome::throws(format!("harness: {error}")))
    }

    #[test]
    fn expand_worktree_template_parity() {
        replay("worktreeLinks-P2/expandWorktreeTemplate", |case: &Case| {
            port(|| {
                Ok(match expand_worktree_template(case.arg_str(0)?, &vars(case, 1)?) {
                    Ok(value) => Outcome::ret(value),
                    Err(error) => Outcome::throws(error),
                })
            })
        });
    }

    #[test]
    fn worktree_location_parity() {
        replay("worktreeLinks-P2/worktreeLocation", |case: &Case| {
            port(|| {
                Ok(match worktree_location(case.arg_str(0)?, &vars(case, 1)?) {
                    Ok(value) => Outcome::ret(value),
                    Err(error) => Outcome::throws(error),
                })
            })
        });
    }

    #[test]
    fn expand_branch_name_parity() {
        replay("worktreeLinks-P2/expandBranchName", |case: &Case| {
            port(|| {
                let bvars: IndexMap<String, String> = case.arg_as(1)?;
                let get = |key: &str| bvars.get(key).map_or("", String::as_str);
                let branch = BranchVars {
                    name: get("name"),
                    user: get("user"),
                };
                Ok(match expand_branch_name(case.arg_str(0)?, &branch) {
                    Ok(value) => Outcome::ret(value),
                    Err(error) => Outcome::throws(error),
                })
            })
        });
    }

    #[test]
    fn branch_name_problem_parity() {
        replay("worktreeLinks-P2/branchNameProblem", |case: &Case| {
            port(|| {
                Ok(match branch_name_problem(case.arg_str(0)?) {
                    Some(problem) => Outcome::ret(problem),
                    None => Outcome::ret(deckhand_testkit::tags::undefined()),
                })
            })
        });
    }

    #[test]
    fn valid_relative_path_parity() {
        replay("worktreeLinks-P2/validRelativePath", |case: &Case| {
            Ok::<_, String>(case.arg(0).as_str().is_some_and(valid_relative_path))
        });
    }

    #[test]
    fn validate_worktree_settings_parity() {
        replay("worktreeLinks-P2/validateWorktreeSettings", |case: &Case| {
            let value = if case.arg_is_undefined(0) {
                Value::Null
            } else {
                deckhand_testkit::tags::untag(case.arg(0))
            };
            match validate_worktree_settings(&value) {
                Ok(settings) => Outcome::ret(settings),
                Err(error) => Outcome::throws(error),
            }
        });
    }

    #[test]
    fn merge_worktree_settings_parity() {
        replay("worktreeLinks-P2/mergeWorktreeSettings", |case: &Case| {
            port(|| {
                let settings = |index: usize| -> std::result::Result<Option<WorktreeSettings>, String> {
                    if case.arg_is_undefined(index) {
                        Ok(None)
                    } else {
                        case.arg_as(index).map(Some)
                    }
                };
                let merged = merge_worktree_settings(settings(0)?.as_ref(), settings(1)?.as_ref());
                Ok(match merged {
                    Some(merged) => Outcome::ret(merged),
                    None => Outcome::ret(deckhand_testkit::tags::undefined()),
                })
            })
        });
    }

    #[test]
    fn files_destination_proto_is_rejected() {
        for key in ["__proto__", "constructor", "prototype"] {
            let mut files = Map::new();
            files.insert(key.to_owned(), Value::String("a".into()));
            let settings = serde_json::json!({ "files": files });
            assert_eq!(
                validate_worktree_settings(&settings).unwrap_err().to_string(),
                format!("Invalid worktree.files destination: \"{key}\"")
            );
        }
    }

    fn file_keys(settings: &WorktreeSettings) -> Vec<&str> {
        settings.files.as_ref().unwrap().keys().map(String::as_str).collect()
    }

    /// TS returns the parsed object itself, whose own keys list array indices first (ascending).
    #[test]
    fn validated_files_follow_js_key_order() {
        let value: Value = serde_json::from_str(r#"{"files":{"b":"/x","1":"/y"}}"#).unwrap();
        let settings = validate_worktree_settings(&value).unwrap();
        assert_eq!(file_keys(&settings), ["1", "b"]);
        assert_eq!(
            serde_json::to_string(&settings).unwrap(),
            r#"{"files":{"1":"/y","b":"/x"}}"#
        );
    }

    /// `{...global.files, ...repository.files}`: indices ascending, then strings in first-insertion order.
    #[test]
    fn merged_files_follow_js_spread_order() {
        let global: WorktreeSettings = serde_json::from_str(r#"{"files":{"b":"/x","10":"/a"}}"#).unwrap();
        let repository: WorktreeSettings = serde_json::from_str(r#"{"files":{"2":"/y","c":"/z","b":"/w"}}"#).unwrap();
        let merged = merge_worktree_settings(Some(&global), Some(&repository)).unwrap();
        assert_eq!(
            serde_json::to_string(&merged).unwrap(),
            r#"{"files":{"2":"/y","10":"/a","b":"/w","c":"/z"}}"#
        );
    }

    #[test]
    fn slugify_user_matches_user_slug() {
        assert_eq!(slugify_user("Émile Zola"), "mile-zola");
        assert_eq!(slugify_user("John.Doe"), "john.doe");
        assert_eq!(slugify_user("..a..b--"), "a.b");
        assert_eq!(slugify_user("---"), "user");
        assert_eq!(slugify_user(""), "user");
    }
}
