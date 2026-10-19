//! Replay helpers for parity fixtures recorded from the TypeScript oracle (see parity/README.md).
//!
//! A fixture is `fixtures/parity/<module>/<fn>.jsonl` (one recorded call per line) or a directory case
//! `fixtures/parity/<module>/<case>/` (before/after trees + op.json). A port test replays it:
//!
//! ```text
//! deckhand_parity::replay("P11", "textEditor/wordLeft", |case| {
//!     Ok::<_, String>(word_left(case.arg_str(0)?, case.arg_usize(1)?))
//! });
//! ```
//!
//! `replay` compares every case's return value / exact error text, honours the unit's allowlist
//! (`parity/units/<unit>.toml`), writes `$PARITY_RESULTS_DIR/<unit>/<fixture>.json`, and panics unless every
//! failure is allowlisted and allowlisted failures are at most 2% of the cases.

pub mod compare;
pub mod dir;
pub mod fixture;
pub mod manifest;
pub mod replay;
pub mod tags;
pub mod utf16;

use std::path::PathBuf;

pub use compare::{diff, diff_with};
pub use dir::{DirOp, Tree, TreeEntry, diff_tree, dir_cases, materialize, replay_dir, serialize_tree};
pub use fixture::{Case, Expect, Fixture};
pub use manifest::{AllowEntry, DeferredTest, Manifest};
pub use replay::{Failure, Outcome, Replay, Report, replay, replay_async};
pub use tags::JsValue;
pub use utf16::{byte_to_utf16, utf16_len, utf16_to_byte};

/// The repository root: `$PARITY_REPO_ROOT`, else two levels above this crate.
pub fn repo_root() -> PathBuf {
    match std::env::var_os("PARITY_REPO_ROOT") {
        Some(root) => PathBuf::from(root),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
    }
}

/// Where fixtures are read from: `$PARITY_FIXTURES_DIR`, else `<repo>/fixtures/parity`.
pub fn fixtures_root() -> PathBuf {
    match std::env::var_os("PARITY_FIXTURES_DIR") {
        Some(root) => PathBuf::from(root),
        None => repo_root().join("fixtures/parity"),
    }
}
