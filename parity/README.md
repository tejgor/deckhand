# Parity fixtures

The fixtures were recorded from the TypeScript app the Rust port started from, which was the oracle until the port
was done. They are now a record of what Deckhand itself does: the tests replay them, and an intended change of
behaviour updates them (below). Current testing policy: [`docs/testing.md`](../docs/testing.md). Manifest format:
`parity/units/README.md`.

## Changing behaviour: blessing

A replay fails when the code no longer returns what a fixture records. When that change is intended:

```bash
DECKHAND_BLESS=1 cargo nextest run -p deckhand-ui -E 'test(panes::settings::)'
git diff -- fixtures/                     # review every changed expectation
cargo nextest run -p deckhand-ui -E 'test(panes::settings::)'
```

Blessing rewrites only the cases that failed, in the fixture's own encoding (tags, UTF-16 offsets, key order): a
`.jsonl` line gets the new `ret`/`throws`/`argsAfter`, a directory case its `op.json` result, `after/` and
`tree.json`. It never blesses a case whose port panicked or an allowlisted case (an input the Rust types cannot hold,
so there is no result to record). Screen and pane snapshots (`deckhand_parity::assert_snapshot`, `fixtures/ui`) bless
the same way. A test that wants a fixture to keep failing (the parity crate's own tests) sets `Replay::bless(false)`.

Blessing only records what the code does. A snapshot cannot tell a fix from a regression, so read the diff, and keep
rules that must hold (every glyph is explained in the guide, an error names the file) as ordinary assertions.

New tests should state the behavior with named inputs and ordinary assertions; they do not need TypeScript or port
manifests. Existing low-churn logic corpora remain useful coverage and can migrate incrementally. Settings grid,
settings rendering and help output have moved to named Rust tests, with focused settings snapshots in
`fixtures/ui/settings`. To extend a remaining replay, add a line `{"args":[...],"ret":null}` (or a directory case
with `before/`, `op.json` and an empty `after` in `tree.json`) and bless only that test. Sections 1 and 3 describe
the historical TS recording and port gates; they are not part of the current development workflow.

```
parity/lib.ts             recorder, tagged encoding, determinism, fast-check helpers, tap driver, directory cases
parity/tap.ts             --import hook: records the calls the existing TS tests make (tap-hooks.mjs does the wrapping)
parity/ts-exports.mjs     TS export / test-title scanner (shared with verify-unit)
parity/gen/<module>.gen.ts  one gen script per src module (template: gen/_example.gen.ts)
fixtures/parity/<module>/<fn>.jsonl      one recorded call per line
fixtures/parity/<module>/<case>/         directory case: before/ after/ tree.json op.json
crates/parity             Rust replay (deckhand-parity, a dev-dependency of every crate)
scripts/verify-unit       per-unit gate
```

## 1. Fixtures agent: adding a module

1. Copy `parity/gen/_example.gen.ts` to `parity/gen/<module>.gen.ts` (`<module>` = `src/<module>.ts` basename).
2. Record, in any mix:
   - **Tap the existing tests** (best corpus, real call shapes):
     `recorder.addTap(await runTap({tests: ['tests/x.test.ts'], tap: ['x:*']}), {module: 'x'})`.
     `tap` entries are `module:fn` or `module:*`. Only calls that cross a module boundary are seen (a module calling
     its own export bypasses the wrapper); class methods are not wrapped. Tests run serially with a temp HOME and
     DECKHAND_HOME; failing tests throw unless `allowFailures`. Manual equivalent:
     `PARITY_TAP=x:fn PARITY_TAP_OUT=<dir> node --import tsx --import ./parity/tap.ts --test tests/x.test.ts`.
   - **Direct calls**: `recorder.call('fn', fn, [args...])` (returns/throws like the call), `await recorder.callAll(...)`
     for async functions, or one-shot `recordCalls(module, 'fn', fn, argLists)`.
   - **fast-check with fixed seeds**: `sample(arbitrary, count, seed?)`; `arb.text()`, `arb.textWithOffset()`,
     `arb.textWithBoundary()` use `EDGE_ALPHABET` (surrogate pairs, combining marks, CJK, newline, tab).
   - **Mutations of recorded inputs**: `mutate(recorder.argsOf('fn'), {variants: 4})` (one leaf changed per variant).
3. `await recorder.write({prune: true})` writes `<module>/<fn>.jsonl` for every recorded function. Files are
   deterministic: lines are de-duplicated, sorted (UTF-16 code-unit order) and capped (default 400, evenly sampled).
   Case index = line number from 0. Object key order inside a line is TS order (never sorted).
4. Call `installDeterminism()` first: fixed clock (`2026-01-01T00:00:00.000Z`, `tickMs` to advance per read), seeded
   `Math.random`, `crypto.randomUUID` (ESM named imports too) and `randomBytes`. Values that still vary (temp paths,
   pids) must be scrubbed: `new Recorder(module, {scrub: line => line.replaceAll(tmp, '$TMP')})`.
5. Regenerate: `node --import tsx parity/gen/<module>.gen.ts`. Run it twice: the second run must not change any byte
   (`git status` clean). `PARITY_FIXTURES_OUT=<dir>` writes elsewhere (verify-unit uses this). A warning about
   "different outcomes" for the same arguments means the function is not deterministic yet.
6. Update `parity/units/<unit>.toml`: `fixtures = ["<module>/<fn>", "<module>/<case>"]` and `utf16_fields`.
7. Type-check the tooling with `npx tsc -p parity`.

### Line format and tags

`{"args":[..], "argsAfter"?:[..], "async"?:true, "ret":..}` or `{.., "throws":"<Error.message verbatim>"}`.
`argsAfter` appears only when the call mutated its arguments (they are encoded before and after the call).
`async` means the function returned a promise; ret/throws are the settled value.

| TS value | Encoding |
|---|---|
| `undefined` (top level, array item, return) | `{"$u":1}` (object properties holding undefined are omitted, as JSON does; `encode: {undefinedProps: 'tag'}` keeps them) |
| `NaN`, `±Infinity` | `{"$num":"NaN"}`, `{"$num":"Infinity"}`, `{"$num":"-Infinity"}` (`-0` records as `0`) |
| `Date` | `{"$date":"<toISOString>"}` (`null` for Invalid Date) |
| `Map`, `Set` | `{"$map":[[k,v],..]}`, `{"$set":[..]}` in insertion order |
| function | `{"$fn":"<name>"}`: name from `registerFn(name, fn)` (else `fn.name`); Rust dispatches on the name |
| xterm Terminal | `{"$screen":{cols, rows, cursor:{x,y}, baseY, viewportY, type, lines:[right-trimmed]}}` |
| Buffer / typed array | `{"$bytes":"<base64>"}` |
| Error value (not thrown) | `{"$error":{"name","message"[,"code"]}}` |
| RegExp, bigint, symbol | `{"$regexp":"/src/flags"}`, `{"$bigint":"12"}`, `{"$symbol":"desc"}` |
| string with lone surrogates | `{"$utf16":[code units]}` |
| cyclic reference | `{"$cycle":1}` |
| plain object whose only key is a tag name | `{"$obj":{..}}` |

### UTF-16 offsets

TS string indices are UTF-16; Rust editor cursors are byte offsets on char boundaries. List every offset field in
the manifest, mapped to the string it indexes. Paths start at the case object; `*` matches every array item / key,
and wildcards bind in order into the string path:

```toml
utf16_fields = { "textEditor/wordLeft" = { "args.1" = "args.0", "ret" = "args.0" },
                 "textEditor/editText" = { "args.0.cursor" = "args.0.text", "ret.cursor" = "ret.text" },
                 "textEditor/wrapRows" = { "ret.*.start" = "args.0", "ret.*.end" = "args.0" } }
```

The replay converts these fields (args, argsAfter and ret) to byte offsets before the port sees them, so the port
works in bytes and returns bytes. Offsets past the end extend linearly, negative ones pass through; an offset inside
a surrogate pair cannot be converted and fails that case. Prefer recording boundary offsets only
(`arb.textWithBoundary`, or filter as `_example.gen.ts` does).

### File-based modules (directory cases)

```ts
await recordDirCase('storage', 'write-json-new-file', {
	setup: async root => { /* build the before state under root */ },
	fn: 'writeJson',                       // what the Rust replay dispatches on
	args: root => [path.join(root, 'a.json'), {x: 1}],
	run: writeJson,                        // the TS function under test
});
```

Writes `fixtures/parity/<module>/<case>/`: `before/` and `after/` (file bytes), `tree.json`
(`{"before":[..], "after":[..]}` entries `{path, type: dir|file|symlink, mode: "0644", target?}`, because Git keeps
neither modes nor empty dirs) and `op.json` (`{fn, args, async?, ret|throws}`). The temp root's path is written as
`$ROOT` everywhere (contents, args, return values, symlink targets). Recording runs with umask 022 and the fixed
clock. Each case is one fixture (`"<module>/<case>"` in the manifest) and must match exactly: every file byte-exact
(JSON included), modes, dirs, symlink targets.

## 2. Port agent: replaying in Rust

```rust
use deckhand_parity::{replay, replay_dir, Case, Outcome, tags};

// Parity replays are ordinary #[test]s in the unit's owned paths (no covers: comment needed).
#[test]
fn word_left_parity() {
    replay("P11", "textEditor/wordLeft", |case: &Case| -> Result<usize, String> {
        Ok(word_left(case.arg_str(0)?, case.arg_usize(1)?))
    });
}
```

- `replay(unit, fixture, port)` loads `parity/units/<unit>.toml` (allowlist, utf16_fields; the fixture must be
  listed), runs every case, writes `$PARITY_RESULTS_DIR/<unit>/<fixture>.json`, and panics with a per-case diff
  unless every failure is allowlisted and allowlisted failures are at most 2% of the cases (`cases / 50`, rounded
  down: under 50 cases nothing may fail). Panics inside `port` fail that case only.
- The port returns anything `Into<Outcome>`: `Result<T: Serialize, E: Display>` (error text compared verbatim with
  the TS `Error.message`), or `Outcome::ret(v)` / `Outcome::throws(msg)`. If the case records `argsAfter`, return
  `Outcome::ret(v).with_args_after(vec![..])` with the arguments after the call.
- Arguments: `case.arg(i)` (tagged `Value`, `{"$u":1}` past the end), `arg_str`, `arg_i64`, `arg_usize`, `arg_f64`
  (handles NaN/Infinity tags), `arg_bool`, `arg_is_undefined`, `fn_name(i)` (for `$fn`), and `arg_as::<T>(i)` which
  deserializes after `tags::untag` (undefined properties dropped, `$date` to its ISO string, `$map` with string keys
  to an object, `$set` to an array...). `tags::decode` gives an exact `JsValue` (Undefined distinct from Null).
- Return values are compared structurally: numbers by value, object key order ignored (`Replay::new(..).key_order(true)`
  for persisted/wire objects), properties holding `{"$u":1}` treated as absent. Build tagged values with
  `tags::undefined()`, `tags::number(f64)`, `tags::date(Some(iso))`, `tags::map(..)`, `tags::set(..)`, `tags::bytes(..)`.
- Async ports: `replay_async(unit, fixture, |case| async move { .. }).await` inside a current_thread runtime.
- Directory cases: `replay_dir(unit, "<module>/<case>", |root: &Path, op: &DirOp| ..)` materializes `before` in a
  temp dir (umask 022), hands over `op.fn_name` and the args with `$ROOT` replaced by that dir (`op` derefs to
  `Case`), then compares the outcome (real root scrubbed back to `$ROOT`) and the whole tree.
  `deckhand_parity::dir_cases(module)` lists a module's cases.
- Need different control (custom manifest, fixtures root, results dir, collecting reports): `Replay::new` /
  `Replay::with_manifest` with `.run`, `.run_async`, `.run_dir`, then `report.assert_ok()`.
- Allowlist a failing case only when the port cannot be judged on it (an input its types cannot represent), with a
  reason (`allowlist = [{ fixture = "...", case = N, reason = "..." }]`); the report lists stale entries. A case
  whose behaviour changed on purpose is blessed instead (see "Changing behaviour").

`crates/parity/tests/example_replay.rs` replays `fixtures/parity/_example` end to end (utf16 offsets, exact error
text, mutation, Map/Set/Date/undefined, `$fn`, directory cases). Its manifest is `crates/parity/tests/example.toml`.

## 3. scripts/verify-unit

```
DECKHAND_HOME=$(mktemp -d) scripts/verify-unit <unit> [--base rust-port] [--gate-suites] [--report out.json] \
    [--only scope,exports] [--skip ts-suites]
```

Refuses to run unless `DECKHAND_HOME` is set and neither equals, is inside, nor contains `~/.deckhand`,
`~/.deckhand-dev`, `~/.codex` or `~/.claude` (symlinks resolved). Children get that DECKHAND_HOME, a fresh temp
HOME, the real CARGO_HOME/RUSTUP_HOME, and `GIT_CONFIG_GLOBAL=/dev/null`. Report (default
`target/verify-unit/<unit>.json`): `{unit, ok, base, head, checks: [{name, ok, gating, details}]}`; exit 0 only when
every gating check passed and none was skipped (`--only`/`--skip` are for iterating), 1 on failure, 2 on usage or
isolation errors.

| check | passes when | details for the fixer |
|---|---|---|
| `scope` | files changed by `git log --no-merges HEAD --not <base> port/<dep>...` plus uncommitted/untracked files are within `rust_paths`, `fixtures/parity/<module>/**`, `parity/gen/<module>.gen.ts`, `parity/units/<unit>.toml`, `Cargo.lock` (modules = ts_sources basenames + fixture modules) | `outside: [{path, source}]` |
| `build` | `cargo build --workspace --all-targets`, `cargo fmt --all --check`, `cargo clippy -p <crate> --all-targets -- -D warnings` | per step: diagnostics with locations, unformatted files, output tail |
| `placeholders` | no `todo!(` / `unimplemented!(` / `#[ignore` in owned files | `hits: [{file, line, text}]` |
| `fixtures` | every manifest fixture exists and `parity/gen/<module>.gen.ts` regenerates its module directory byte-identically | stale / uncommitted / differing files with the first differing line |
| `tests` | `cargo nextest run -p <crate>` (else `cargo test`) passes; every manifest fixture has a result file; each result has no unallowlisted failure and allowlisted <= 2% | failing tests, `failing_cases: [{index, kind, diff, args}]`, `not_replayed` |
| `exports` | every export of every ts_source (types included) is mapped in exactly one manifest's `[exports]` to an existing Rust symbol, or `"drop: reason"`; no stale keys | `missing`, `duplicates`, `invalid` (with the problem), `stale` |
| `ts-tests` | every `test(`/`it(` title in ts_tests is covered by a Rust test in owned paths with `// covers: B-<unit>-NN <exact title>` directly above its `#[test]` that passed in `tests`, or listed in `deferred_tests` | `missing: [{title, file, line}]`, `not_passed`, `problems`, next free NN |
| `ts-suites` | `node --import tsx --test <ts_suites>` passes twice with `DECKHAND_TEST_BIN=target/debug/deckhand` | per run pass/fail counts and failing titles; gating only with `--gate-suites` |
