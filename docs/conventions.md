# Code conventions

The rules the Rust code follows. Testing policy is in [testing.md](testing.md).

## Isolation (hard rules)

- Never read, write, or run anything against `~/.deckhand`, `~/.deckhand-dev`, `~/.codex`, `~/.claude`, or shell rc
  files from a test or a development run. Use the dev launcher ([dev-build.md](dev-build.md)) for manual runs.
- Every test runs with a temp `DECKHAND_HOME` **and** a temp `HOME` created under `$TMPDIR`.
- No `cargo install` while testing a branch. Binaries run from `target/`. `setup`/`doctor` only with `--check`.
- Never start the real `claude`, `codex` or `pi` from a test. Use the fake agent from `crates/e2e`.
- Debug builds of `deckhand` refuse `~/.deckhand` unless `DECKHAND_ALLOW_PROD_HOME=1` (`crates/deckhand/src/guard.rs`).

## Runtime model

- Every process: tokio `current_thread` runtime + `LocalSet`. Shared state is `Rc<RefCell<..>>`; never hold a
  `Ref`/`RefMut` across `.await` (`clippy::await_holding_refcell_ref` is denied). Pattern: borrow, copy what you need,
  drop, then await.
- Async coordination lives in `deckhand_sys::async_util`: `KeyedQueue` (per-key serial queue, e.g. per file / per
  workspace), `Coalescer` ("run once more after the current run", as `persist()` needs), `SharedSlot` (dedupe
  in-flight work, every caller awaits the same result), `Timer` (cancel on drop).
- Child processes: `deckhand_sys::proc` (exec with timeout, `setsid` so pgid == pid, `killpg` on timeout/drop).
- No `unwrap()`/`expect()`/indexing panics on untrusted input (files, sockets, PTY output, git output, argv, env).

## Strings

- Lengths users see as limits, and lengths that are persisted, use UTF-16 semantics, as Deckhand always has:
  `deckhand_core::jsstr` (`js_len`, `js_slice`, `js_substring`, `locale_compare`, ...). Truncation widths are display
  width (below).
- Editor cursors are byte offsets on char boundaries.
- Terminal display width: `unicode-width`, matching the vectors in `crates/ui/tests/data`.
- Sorting names users see: `jsstr::locale_compare`, never plain `cmp`.

## Errors

- `deckhand_core::error::DhError` (thiserror). Messages carry the useful context (which file, which setting); keep
  exact wording only where something depends on it (a test that states the rule, a format, users' scripts).

## Serde, wire and persisted formats

- `serde_json` with `preserve_order`. Existing formats keep their key order, especially where bytes feed a hash or
  revision.
- `#[serde_with::skip_serializing_none]` on structs; `Option<Option<T>>` with `serde_with::rust::double_option` for
  "absent vs null".
- Persisted records carry `#[serde(flatten)] extra: IndexMap<String, Value>` so unknown keys round-trip (an older or
  newer Deckhand may have written them).
- String unions: enums with explicit `#[serde(rename = "...")]` on every variant.
- Files are written with `deckhand_sys::storage` atomic writes (temp + rename), modes 0600 files / 0700 dirs.
- `PROTOCOL_VERSION` is defined in `crates/protocol`; bump it on any request/response shape change.

## Tests

- Name each test for the rule it protects and assert the result that matters ([testing.md](testing.md)).
- Git-based tests use a `repo()` helper: `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_NOSYSTEM=1`, `-b main`, a fixed
  user, `commit.gpgsign=false`.
- No `todo!()`, `unimplemented!()` or `#[ignore]` in finished work.

## Comments

Explain why, not what. Many modules still note the TypeScript function they were ported from (`TS: ...`); that is
history, not a requirement, and new code does not add such references.
