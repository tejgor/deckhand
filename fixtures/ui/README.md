# UI screen snapshots

What the app draws, as text: one line per terminal row, trailing spaces trimmed. Tests render the app (or one pane)
with ratatui's `TestBackend` and compare with `deckhand_testkit::assert_snapshot`; an intended change is updated with
`DECKHAND_BLESS=1` (see `fixtures/cases/README.md`, "Changing behaviour"), then reviewed in the diff.

- `<nn>-<name>.txt`, used by `crates/ui/src/app/layout.rs`: the whole 130x36 screen.
- `settings/<name>.txt`: named settings scenarios rendered directly from Rust inputs, including selected styles,
  pending trust and narrow-column or choice-editor states.
- `panes/<nn>-<name>.txt`: the right pane of the 130x36 app (98x31) drawn by `app/modes/confirm.rs`,
  `app/modes/create.rs`, `app/modes/action_picker.rs` and `flows/tasks.rs`, at an explicit 98x31 size. A tasks
  snapshot also holds the footer
  hint (after `── hint`) and the styled runs of every row (after `── styles`: `start..end` in columns, then `fg`/`bg`
  and the modifiers that are set).
