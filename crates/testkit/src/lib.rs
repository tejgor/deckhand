//! Test helpers shared by the workspace: replaying the recorded fixtures in `fixtures/cases`, text snapshots
//! (`fixtures/ui`), and proptest strategies. See `fixtures/cases/README.md` and `docs/testing.md`.
//!
//! A fixture is `fixtures/cases/<module>/<fn>.jsonl` (one recorded call per line) or a directory case
//! `fixtures/cases/<module>/<case>/` (before/after trees + op.json). A test replays it through the code:
//!
//! ```text
//! deckhand_testkit::replay("textEditor/wordLeft", |case| {
//!     Ok::<_, String>(word_left(case.arg_str(0)?, case.arg_usize(1)?))
//! });
//! ```
//!
//! `replay` compares every case's return value / exact error text, honours the allowlist in
//! `fixtures/cases/cases.toml`, and panics unless every failure is allowlisted and allowlisted failures are at most 2%
//! of the cases. When behaviour changes on purpose, `DECKHAND_BLESS=1` rewrites the failing cases instead;
//! [`snapshot`] does the same for rendered text.

pub mod compare;
pub mod dir;
pub mod fixture;
pub mod replay;
pub mod settings;
pub mod snapshot;
pub mod strategies;
pub mod tags;
pub mod utf16;

use std::path::PathBuf;

pub use compare::{diff, diff_with};
pub use dir::{DirOp, Tree, TreeEntry, diff_tree, dir_cases, materialize, replay_dir, serialize_tree};
pub use fixture::{Case, Expect, Fixture};
pub use replay::{Failure, Outcome, Replay, Report, replay, replay_async};
pub use settings::{AllowEntry, CaseSettings};
pub use snapshot::assert_snapshot;
pub use tags::JsValue;
pub use utf16::{byte_to_utf16, utf16_len, utf16_to_byte};

/// `DECKHAND_BLESS` is set (and not `0`): a replay or snapshot that differs rewrites its fixture from what the code
/// produced instead of failing. Run the tests with it after an intended change, then review the fixture diff.
pub fn blessing() -> bool {
    std::env::var_os("DECKHAND_BLESS").is_some_and(|value| !value.is_empty() && value != "0")
}

/// The repository root (two levels above this crate).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where the replayed fixtures live: `<repo>/fixtures/cases`.
pub fn fixtures_root() -> PathBuf {
    repo_root().join("fixtures/cases")
}
