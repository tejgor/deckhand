# Recorded cases

Inputs and expected results for Deckhand's pure logic and file operations, replayed by ordinary Rust tests through
`deckhand_testkit` (`crates/testkit`). They were first recorded from the TypeScript app Deckhand was ported from;
they now record what Deckhand itself does, and an intended change updates them. New behaviour is better tested with
named tests and assertions ([docs/testing.md](../../docs/testing.md)); these files are a regression corpus for edge
cases that would be tedious to write by hand.

- `<module>/<fn>.jsonl`: one call per line.
- `<module>/<case>/`: a directory case (a file operation and the files before and after it).
- `cases.toml`: the UTF-16 offset fields and the allowlist (below).

## Replaying

```rust
use deckhand_testkit::{replay, replay_dir, Case, Outcome, tags};

#[test]
fn word_left_cases() {
    replay("textEditor/wordLeft", |case: &Case| -> Result<usize, String> {
        Ok(word_left(case.arg_str(0)?, case.arg_usize(1)?))
    });
}
```

- `replay(fixture, f)` runs every case, compares, and panics with a per-case diff unless every failure is allowlisted
  and allowlisted failures are at most 2% of the cases (`cases / 50`, rounded down). A panic inside `f` fails that
  case only.
- `f` returns anything `Into<Outcome>`: `Result<T: Serialize, E: Display>` (the error's text is compared verbatim
  with `throws`), or `Outcome::ret(v)` / `Outcome::throws(msg)`. When the case records `argsAfter`, return
  `Outcome::ret(v).with_args_after(vec![..])` with the arguments after the call.
- Arguments: `case.arg(i)` (tagged `Value`, `{"$u":1}` past the end), `arg_str`, `arg_i64`, `arg_usize`, `arg_f64`
  (handles the NaN/Infinity tags), `arg_bool`, `arg_is_undefined`, `fn_name(i)` (for `$fn`), and `arg_as::<T>(i)`,
  which deserializes after `tags::untag`. `tags::decode` gives an exact `JsValue` (undefined distinct from null).
- Results are compared structurally: numbers by value, object key order ignored (`Replay::new(..).key_order(true)`
  for persisted and wire objects), properties holding `{"$u":1}` treated as absent.
- Async: `replay_async(fixture, |case| async move { .. }).await` inside a current_thread runtime.
- Directory cases: `replay_dir("<module>/<case>", |root: &Path, op: &DirOp| ..)` materializes `before` in a temp dir
  (umask 022), hands over `op.fn_name` and the args with `$ROOT` replaced by that dir, then compares the outcome and
  the whole resulting tree. `deckhand_testkit::dir_cases(module)` lists a module's cases.
- Finer control (other settings, another fixtures root, collecting reports): `Replay::new` /
  `Replay::with_settings` with `.run`, `.run_async`, `.run_dir`, then `report.assert_ok()`.

`crates/testkit/tests/example_replay.rs` replays the small `_example` corpus beside it end to end.

## Changing behaviour: blessing

A replay fails when the code no longer returns what a case records. When the change is intended:

```bash
DECKHAND_BLESS=1 cargo nextest run -p deckhand-core -E 'test(text_editor::)'   # or: cargo test -p deckhand-core text_editor
git diff -- fixtures/                                                           # review every changed case
cargo nextest run -p deckhand-core -E 'test(text_editor::)'
```

Blessing rewrites only the cases that failed, in the file's own encoding (tags, UTF-16 offsets, key order): a line
gets the new `ret`/`throws`/`argsAfter`, a directory case its `op.json` result, `after/` and `tree.json`. It never
blesses a case whose code panicked, or an allowlisted case. Text snapshots (`assert_snapshot`, `fixtures/ui`) bless
the same way. A test that wants a case to keep failing (the testkit's own tests) uses `Replay::bless(false)`.

Blessing records what the code does; it cannot tell a fix from a regression. Read the diff, and keep rules that must
hold as ordinary assertions. To add a case, append a line `{"args":[...],"ret":null}` (or a directory case with
`before/`, `op.json` and an empty `after` in `tree.json`) and bless that test.

## Line format and tags

`{"args":[..], "argsAfter"?:[..], "async"?:true, "ret":..}` or `{.., "throws":"<error text>"}`. `argsAfter` appears
only when the call mutates its arguments. `async`: the result is a promise's settled value. Case index = line number
from 0 (what the allowlist names). Values JSON cannot hold are tagged:

| Value | Encoding |
|---|---|
| undefined (top level, array item, return) | `{"$u":1}` (object properties holding it are omitted) |
| `NaN`, `±Infinity` | `{"$num":"NaN"}`, `{"$num":"Infinity"}`, `{"$num":"-Infinity"}` |
| date | `{"$date":"<ISO string>"}` (`null` for an invalid date) |
| map, set | `{"$map":[[k,v],..]}`, `{"$set":[..]}` in insertion order |
| function | `{"$fn":"<name>"}`; the test dispatches on the name |
| terminal screen | `{"$screen":{cols, rows, cursor:{x,y}, baseY, viewportY, type, lines:[right-trimmed]}}` |
| bytes | `{"$bytes":"<base64>"}` |
| error value (not thrown) | `{"$error":{"name","message"[,"code"]}}` |
| regexp, bigint, symbol | `{"$regexp":"/src/flags"}`, `{"$bigint":"12"}`, `{"$symbol":"desc"}` |
| string with lone surrogates | `{"$utf16":[code units]}` |
| cyclic reference | `{"$cycle":1}` |
| plain object whose only key is a tag name | `{"$obj":{..}}` |

## UTF-16 offsets

The recorded string offsets are UTF-16 code-unit indices; Deckhand's editor cursors are byte offsets on char
boundaries. `cases.toml` lists every offset field per fixture, mapped to the string it indexes. Paths start at the
case object; `*` matches every array item or key, and wildcards bind in order into the string path:

```toml
[utf16_fields]
"textEditor/wordLeft" = { "args.1" = "args.0", "ret" = "args.0" }
"textEditor/editText" = { "args.0.cursor" = "args.0.text", "ret.cursor" = "ret.text" }
"textEditor/wrapRows" = { "ret.*.start" = "args.0", "ret.*.end" = "args.0" }
```

The replay converts these fields (args, argsAfter and ret) to byte offsets before the test sees them, and back when
blessing. Offsets past the end extend linearly, negative ones pass through; an offset inside a surrogate pair cannot
be converted and fails that case.

## Allowlist

`[[allowlist]]` entries in `cases.toml` (`fixture`, `case`, `reason`) mark cases the code cannot be judged on, such as
an input its types cannot represent (an enum value outside the type, a lone surrogate). They may fail and are never
blessed; the report lists entries that pass again as stale. A case whose behaviour changed on purpose is blessed, not
allowlisted.

## Directory cases

`<module>/<case>/` holds `before/` and `after/` (file bytes), `tree.json` (`{"before":[..], "after":[..]}` entries
`{path, type: dir|file|symlink, mode: "0644", target?}`, because Git keeps neither modes nor empty dirs) and `op.json`
(`{fn, args, async?, ret|throws}`). The temp root's path is written as `$ROOT` everywhere (contents, args, results,
symlink targets). Every file must match byte for byte, with its mode, every dir and every symlink target.
