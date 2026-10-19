# Unit manifests

One `<unit>.toml` per port unit. It says what the unit owns and how `scripts/verify-unit <unit>` judges it
(see `parity/README.md`). A unit edits only its own manifest.

```toml
id = "P11"                       # file name without .toml
wave = 1
title = "text editor, terminal keys, config draft"
crate = "core"                   # crates/<crate>; clippy/tests run on its package
ts_sources = ["src/textEditor.ts"]           # oracle modules; module name = basename (textEditor)
rust_paths = ["crates/core/src/text_editor.rs"]   # files (or dirs) this unit owns; nothing else may change
deps = []                        # units whose port/<dep> branches are merged in (excluded from the scope check)
ts_tests = ["tests/textEditor.test.ts"]      # every test title must be covered or deferred
ts_suites = []                   # TS suites run against target/debug/deckhand (gating from Wave 4)

fixtures = ["textEditor/wordLeft", "storage/write-json-new-file"]   # fixtures/parity/<name>.jsonl or <name>/op.json
utf16_fields = { "textEditor/wordLeft" = { "args.1" = "args.0", "ret" = "args.0" } }

deferred_tests = [
  { title = "<exact TS test title>", to = "U3", reason = "needs the Ink renderer" },   # to = <unit> or "drop"
]
allowlist = [
  { fixture = "textEditor/wordLeft", case = 17, reason = "ICU word-break difference" },  # case = line from 0
]

[exports]
# <module>.<export> = "<repo path of .rs>::<symbol>"  or  "drop: <reason>"; types and constants included.
textEditor.editText = "crates/core/src/text_editor.rs::edit_text"
textEditor.EditorState = "crates/core/src/text_editor.rs::EditorState"
textEditor.EditOptions = "crates/core/src/text_editor.rs::EditOptions"
textEditor.lineStart = "drop: private helper in Rust, behaviour covered through edit_text fixtures"

[internals]
# Optional, not checked: non-exported TS functions/consts mapped for traceability, same value format.
"daemon.handleRequest.ping" = "crates/daemon/src/daemon.rs::Daemon::handle_ping"
```

Rules verify-unit enforces:

- `[exports]`: every export of every `ts_sources` file (functions, consts, classes, enums, interfaces, types, default)
  appears in exactly one manifest across `parity/units/` (another unit may map it, e.g. shared types in protocol).
  `"<file.rs>::<symbol>"`: the file must exist and define the symbol (`fn|struct|enum|trait|type|const|static|mod`);
  `Type::method` / `Enum::Variant` paths are allowed (later segments only need to occur in the file). A list of
  targets is allowed when one export becomes several items. Keys naming no export are errors.
- Rust tests porting a TS test: `// covers: B-<unit>-NN <exact TS title>` on the line(s) directly above the test
  attributes, NN from 01, unique per unit. The test must pass in the `tests` check.
- `deferred_tests`: title must exist in `ts_tests`, `to` must be `"drop"` or an existing unit, reason required.
- `allowlist`: at most 2% of a fixture's cases (rounded down), each with a reason; entries that no longer fail are
  reported as stale.
- `utf16_fields`: see parity/README.md "UTF-16 offsets".
