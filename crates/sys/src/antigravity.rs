//! What Deckhand reads of Antigravity's (`agy`'s) own state: the last conversation it had in a directory.
//!
//! agy names its conversation only when it exits (`agy --conversation=<id>`). A session the daemon could not watch
//! exit (`deckhand restart`, a daemon crash, a reboot) has no ID, so resuming it falls back to
//! `<app data>/cache/last_conversations.json`, which maps each directory agy ran in to its latest conversation. agy
//! writes it at a conversation's first message. The entry names the directory's latest conversation whoever had it
//! (agy run by hand there, or a conversation the session left with `S`), so it is taken only when that
//! conversation's file (`conversations/<id>.db`, written as it is used) changed during the session's last launch.
//! Two agy sessions in one directory overwrite each other's entry, so the caller also uses it only when no other agy
//! session shares that directory. Nothing here is ever written.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use serde_json::Value;

use crate::paths::home_dir;

/// The variable agy sets for what it runs, naming its data directory; honoured when the daemon has it too.
const APP_DATA_VAR: &str = "ANTIGRAVITY_APP_DATA_DIR";

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        .expect("UUID is a valid regex")
});

/// agy's data directory: `$ANTIGRAVITY_APP_DATA_DIR`, else `~/.gemini/antigravity-cli`.
pub fn app_data_dir() -> PathBuf {
    match std::env::var_os(APP_DATA_VAR).filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home_dir().join(".gemini").join("antigravity-cli"),
    }
}

/// The conversation agy last had in `cwd` (as given, else its canonical path: agy keys by the path it ran in, and
/// under Deckhand that is the canonical one), when its file was written at or after `since` (the launch's
/// start). `None` when the index is missing or unreadable, holds no UUID for it, or that conversation was not used
/// since then.
pub async fn last_conversation(data_dir: &Path, cwd: &str, since: SystemTime) -> Option<String> {
    let text = tokio::fs::read_to_string(data_dir.join("cache").join("last_conversations.json"))
        .await
        .ok()?;
    let canonical = tokio::fs::canonicalize(cwd)
        .await
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    let id = conversation_for(&text, cwd, canonical.as_deref())?;
    let written = tokio::fs::metadata(data_dir.join("conversations").join(format!("{id}.db")))
        .await
        .ok()?
        .modified()
        .ok()?;
    (written >= since).then_some(id)
}

/// [`last_conversation`] on the file's text.
fn conversation_for(text: &str, cwd: &str, canonical: Option<&str>) -> Option<String> {
    let entries: Value = serde_json::from_str(text).ok()?;
    [Some(cwd), canonical]
        .into_iter()
        .flatten()
        .filter_map(|path| entries.get(path)?.as_str())
        .find(|id| UUID.is_match(id))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const ID: &str = "93794b97-c054-43d9-9250-97bc6b6cd2b9";

    #[test]
    fn reads_the_directorys_conversation_by_its_path_or_canonical_path() {
        let text = format!(r#"{{"/w/a": "{ID}", "/private/tmp/b": "{ID}", "/w/bad": "--settings=x"}}"#);
        assert_eq!(conversation_for(&text, "/w/a", None).as_deref(), Some(ID));
        assert_eq!(
            conversation_for(&text, "/tmp/b", Some("/private/tmp/b")).as_deref(),
            Some(ID)
        );
        assert_eq!(conversation_for(&text, "/w/other", None), None);
        // Only a UUID: never something that would become a flag.
        assert_eq!(conversation_for(&text, "/w/bad", None), None);
        assert_eq!(conversation_for("not json", "/w/a", None), None);
        assert_eq!(conversation_for("[]", "/w/a", None), None);
    }

    #[test]
    fn takes_the_directorys_conversation_only_when_it_was_used_since_the_launch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = tempfile::tempdir().expect("work dir");
        let cwd = work.path().to_string_lossy().into_owned();
        let canonical = std::fs::canonicalize(work.path()).expect("canonical");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let launch = SystemTime::now() - Duration::from_secs(60);
        let read = |since| runtime.block_on(last_conversation(dir.path(), &cwd, since));
        assert_eq!(read(launch), None);
        std::fs::create_dir_all(dir.path().join("cache")).expect("cache dir");
        std::fs::write(
            dir.path().join("cache/last_conversations.json"),
            serde_json::json!({ canonical.to_string_lossy(): ID }).to_string(),
        )
        .expect("write");
        // Indexed, but its conversation file is missing: not taken.
        assert_eq!(read(launch), None);
        std::fs::create_dir_all(dir.path().join("conversations")).expect("conversations dir");
        let db = dir.path().join(format!("conversations/{ID}.db"));
        std::fs::write(&db, "").expect("db");
        assert_eq!(read(launch).as_deref(), Some(ID));
        // Last written before the launch (agy run by hand earlier, a launch without a message, a conversation left
        // with S): a stale entry, not taken.
        let file = std::fs::File::options().write(true).open(&db).expect("open db");
        file.set_modified(launch - Duration::from_secs(3600))
            .expect("set mtime");
        assert_eq!(read(launch), None);
    }
}
