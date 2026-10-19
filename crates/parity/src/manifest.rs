//! Unit manifests: `parity/units/<unit>.toml` (format in parity/units/README.md).

use std::path::Path;

use indexmap::IndexMap;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub wave: i64,
    pub title: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub ts_sources: Vec<String>,
    pub rust_paths: Vec<String>,
    pub deps: Vec<String>,
    pub ts_tests: Vec<String>,
    pub ts_suites: Vec<String>,
    /// `<module>/<fn>` (jsonl) or `<module>/<case>` (directory case) under fixtures/parity.
    pub fixtures: Vec<String>,
    /// fixture -> { offset path -> string path } (see `utf16`).
    pub utf16_fields: IndexMap<String, IndexMap<String, String>>,
    pub deferred_tests: Vec<DeferredTest>,
    pub allowlist: Vec<AllowEntry>,
    /// `<module>.<export>` -> `"<file.rs>::<symbol>"` or `"drop: <reason>"` (kept raw; verify-unit checks it).
    pub exports: toml::Table,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DeferredTest {
    pub title: String,
    pub to: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AllowEntry {
    pub fixture: String,
    pub case: usize,
    pub reason: String,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| format!("invalid unit manifest: {error}"))
    }

    pub fn load_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// `parity/units/<unit>.toml` in the repository.
    pub fn load(unit: &str) -> Result<Self, String> {
        Self::load_path(&crate::repo_root().join("parity/units").join(format!("{unit}.toml")))
    }

    /// Allowlisted case indices of `fixture`, with reasons.
    pub fn allowed(&self, fixture: &str) -> IndexMap<usize, String> {
        self.allowlist
            .iter()
            .filter(|entry| entry.fixture == fixture)
            .map(|entry| (entry.case, entry.reason.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_skeleton_manifests() {
        let dir = crate::repo_root().join("parity/units");
        let mut count = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "toml") {
                let manifest = Manifest::load_path(&path).unwrap();
                assert!(!manifest.id.is_empty(), "{}", path.display());
                count += 1;
            }
        }
        assert!(count > 0);
    }

    #[test]
    fn parses_fixtures_allowlist_and_utf16_fields() {
        let manifest = Manifest::parse(
            r#"
id = "X1"
crate = "core"
fixtures = ["m/f"]
utf16_fields = { "m/f" = { "args.1" = "args.0", "ret" = "args.0" } }
allowlist = [{ fixture = "m/f", case = 3, reason = "ICU difference" }]
deferred_tests = [{ title = "t", to = "drop", reason = "UI only" }]
[exports]
m.f = "crates/core/src/m.rs::f"
"#,
        )
        .unwrap();
        assert_eq!(manifest.crate_name, "core");
        assert_eq!(manifest.utf16_fields["m/f"]["ret"], "args.0");
        assert_eq!(
            manifest.allowed("m/f").get(&3).map(String::as_str),
            Some("ICU difference")
        );
        assert_eq!(manifest.deferred_tests[0].to, "drop");
        assert!(
            manifest.exports["m"]
                .as_table()
                .is_some_and(|table| table.contains_key("f"))
        );
    }
}
