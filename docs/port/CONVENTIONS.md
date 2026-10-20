# Rust port conventions

The rules the port was written under. During the port the TypeScript tree (`src/`, `tests/`) was the **oracle**:
behaviour, error text, timing and on-disk formats had to match it. The port is done, and Deckhand's own behaviour is
now the reference: the fixtures and snapshots record it, and an intended change updates them with
`DECKHAND_BLESS=1` (see `parity/README.md`, "Changing behaviour"). What still holds from the TS: a state directory it
wrote must keep loading (`fixtures/state` is frozen input, never blessed). The isolation rules below still apply; the
ownership and unit rules were for the port's parallel units.

Current development and testing policy lives in [`docs/testing.md`](../testing.md). New Rust tests need neither TS
test-title markers nor a port-unit manifest. The remaining sections document the port's implementation constraints;
they do not impose blanket TS output equivalence on new behavior.

## Isolation (hard rules)
- Never read, write, or run anything against `~/.deckhand`, `~/.deckhand-dev`, `~/.codex`, `~/.claude`, or shell rc files.
- Every test (TS or Rust) runs with a temp `DECKHAND_HOME` **and** a temp `HOME` created under `$TMPDIR`.
- No `cargo install`, `npm link`, `npm i -g`. Binaries run from `target/` only. `setup`/`doctor` only with `--check`.
- Never start the real `claude`, `codex` or `pi`. Use the fake agent from `tests/helpers.ts` / `crates/e2e`.
- Never push. Commit only on the branch you were told to use.
- Debug builds of `deckhand` refuse `~/.deckhand` unless `DECKHAND_ALLOW_PROD_HOME=1` (`crates/deckhand/src/guard.rs`).

## Ownership
- Each unit owns exactly the files in `parity/units/<unit>.toml` `rust_paths`. Edit nothing else. `lib.rs`/`mod.rs`
  module lists, `Cargo.toml` files and other units' files are frozen; if you truly need a change there, stop and
  report it in your result instead of making it.
- Public signatures frozen by the Wave 0 skeleton may only change inside the owning unit if no other crate uses them
  yet; otherwise report the needed change.

## Runtime model
- Every process: tokio `current_thread` runtime + `LocalSet`. Shared state is `Rc<RefCell<..>>`; never hold a
  `Ref`/`RefMut` across `.await` (`clippy::await_holding_refcell_ref` is denied). Pattern: borrow, copy what you need,
  drop, then await.
- JS promise-chain idioms map to `deckhand_sys::async_util`: `KeyedQueue` (per-key serial queue, e.g. per file / per
  workspace), `Coalescer` (`persist()`-style "run once more after the current run"), `SharedSlot` (dedupe in-flight
  work, every caller awaits the same result), `Timer` (`setTimeout`/`clearTimeout`, cancel on drop).
- Child processes: `deckhand_sys::proc` (exec with timeout, `setsid` so pgid == pid, `killpg` on timeout/drop).
- No `unwrap()`/`expect()`/indexing panics on untrusted input (files, sockets, PTY output, git output, argv, env).

## Strings
- TS strings are UTF-16. Anything the user sees as a length limit or that is persisted (truncation widths are
  display-width, see below) uses `deckhand_core::jsstr` (`js_len`, `js_slice`, `js_substring`, `locale_compare`, ...).
- Editor cursors are byte offsets on char boundaries in Rust; fixtures record UTF-16 offsets and the manifest
  `utf16_fields` lists which fields to convert.
- Terminal display width: `unicode-width`, matching Ink's `string-width` behaviour as recorded in fixtures.
- `localeCompare` → `jsstr::locale_compare` (never plain `cmp` where TS used `localeCompare`).

## Errors
- `deckhand_core::error::DhError` (thiserror). During the port its `Display` matched TS errors verbatim. Current
  tests should assert the useful error and context; preserve exact wording where an explicit contract requires it.

## Serde / wire / persisted formats
- `serde_json` with `preserve_order`. Existing formats retain their ordering, especially where bytes affect a hash
  or revision; the port's general byte-equivalence rule is not a requirement for new formats.
- `#[serde_with::skip_serializing_none]` on every struct; `Option<Option<T>>` with `serde_with::rust::double_option`
  where TS has `?: T | null`.
- Persisted records carry `#[serde(flatten)] extra: IndexMap<String, Value>` so unknown keys round-trip.
- String unions: enums with explicit `#[serde(rename = "...")]` on every variant.
- Files written with `deckhand_sys::storage` atomic write (temp + rename), modes 0600 files / 0700 dirs as TS does.
- The current `PROTOCOL_VERSION` is defined in `crates/protocol`; bump it on any request/response shape change.

## Tests
- Port each TS `test('title', ...)` from the unit's `ts_tests` to a Rust `#[test]` (or `#[tokio::test(flavor =
  "current_thread")]`) with a comment line directly above: `// covers: B-<unit>-NN <exact TS title>`; NN counts from 01.
- Parity fixtures (`fixtures/parity/<module>/<fn>.jsonl`) are replayed by a test in the owning unit using
  the `deckhand-parity` crate (dev-dependency of every crate) (see `parity/README.md`).
- No `todo!()`, `unimplemented!()`, `#[ignore]` in owned paths when a unit is done.
- Git-based tests use a `repo()` helper equivalent to `tests/helpers.ts` (GIT_CONFIG_GLOBAL=/dev/null,
  GIT_CONFIG_NOSYSTEM=1, `-b main`, fixed user, `commit.gpgsign=false`).

## Verification
`scripts/verify-unit <unit>` writes a JSON report and exits non-zero on failure. Run it before you finish and include
the report path in your result.

## File headers
Every Rust module starts with `//! Port of <ts file(s)>.` Keep comment density similar to the TS source: explain
why, not what.
