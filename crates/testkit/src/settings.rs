//! `fixtures/cases/cases.toml`: what a replay needs to know about a fixture beyond its cases, namely which fields hold
//! UTF-16 offsets and which cases are allowlisted.

use std::path::Path;
use std::sync::LazyLock;

use indexmap::IndexMap;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CaseSettings {
    /// fixture -> { offset path -> string path } (see `utf16`).
    pub utf16_fields: IndexMap<String, IndexMap<String, String>>,
    /// Cases the code cannot be judged on (an input its types cannot represent): they may fail, and are never blessed.
    pub allowlist: Vec<AllowEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AllowEntry {
    pub fixture: String,
    pub case: usize,
    pub reason: String,
}

static SHARED: LazyLock<Result<CaseSettings, String>> =
    LazyLock::new(|| CaseSettings::load_path(&crate::fixtures_root().join("cases.toml")));

impl CaseSettings {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| format!("invalid case settings: {error}"))
    }

    pub fn load_path(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// `fixtures/cases/cases.toml`, read once.
    pub fn shared() -> Result<Self, String> {
        SHARED.clone()
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
    fn the_repository_settings_parse_and_name_existing_fixtures() {
        let settings = CaseSettings::shared().unwrap();
        assert!(!settings.utf16_fields.is_empty());
        let root = crate::fixtures_root();
        for fixture in settings
            .utf16_fields
            .keys()
            .chain(settings.allowlist.iter().map(|entry| &entry.fixture))
        {
            assert!(
                root.join(format!("{fixture}.jsonl")).is_file() || root.join(fixture).join("op.json").is_file(),
                "{fixture} is not a fixture"
            );
        }
    }

    #[test]
    fn parses_utf16_fields_and_the_allowlist() {
        let settings = CaseSettings::parse(
            r#"
[utf16_fields]
"m/f" = { "args.1" = "args.0", "ret" = "args.0" }

[[allowlist]]
fixture = "m/f"
case = 3
reason = "cannot be represented"
"#,
        )
        .unwrap();
        assert_eq!(settings.utf16_fields["m/f"]["ret"], "args.0");
        assert_eq!(
            settings.allowed("m/f").get(&3).map(String::as_str),
            Some("cannot be represented")
        );
        assert!(CaseSettings::parse("id = \"X1\"").is_err());
    }
}
