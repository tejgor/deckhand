# Parity allowlist (gate G2 — for user review)

Fixture cases where the Rust port deliberately differs from the TS oracle. verify-unit allows at most 2% of a fixture's cases, each with a reason. Every other recorded case matches exactly.

| Unit | Fixture | Case | Reason |
|---|---|---|---|
| I7 | settingsInfo/codexHookCommands | 77 | JSON escape \ud83d without its pair: V8 keeps a lone surrogate, Rust strings cannot hold one (read as U+FFFD) |
| I7 | settingsInfo/codexHookCommands | 78 | JSON escape \ud83d without its pair: V8 keeps a lone surrogate, Rust strings cannot hold one (read as U+FFFD) |
| I7 | settingsInfo/codexHookCommands | 79 | JSON escape \ud83d without its pair: V8 keeps a lone surrogate, Rust strings cannot hold one (read as U+FFFD) |
| I7 | settingsInfo/codexHookCommands | 80 | JSON escape \ud83d without its pair: V8 keeps a lone surrogate, Rust strings cannot hold one (read as U+FFFD) |
| P10 | projectConfig-P10/parseProjectConfig | 36 | a lone surrogate escape (\ud83d) cannot live in a Rust String; it becomes U+FFFD, what Node writes for it as UTF-8 |
| P10 | projectConfig-P10/parseProjectConfig | 37 | a lone surrogate escape (\ud83d) cannot live in a Rust String; it becomes U+FFFD, what Node writes for it as UTF-8 |
| P11 | textEditor/wordLeft | 95 | mutated cursor -1: a negative UTF-16 offset has no byte-offset (usize) form in the Rust API |
| P11 | textEditor/wordRight | 94 | mutated cursor -1: a negative UTF-16 offset has no byte-offset (usize) form in the Rust API |
| P2 | worktreeLinks-P2/validateWorktreeSettings | 89 | recording artifact: the TS input {files: JSON.parse('{"__proto__": "a"}')} has an own __proto__ key the fixture encoder writes as {files: {}}; the Rust rejection of that key is checked by worktree_links::tests::files_destination_proto_is_rejected |
| P3 | notes/toggleChecklist | 351 | cursor -1 is not a byte offset: EditorState.cursor is usize, so the replay passes 0 and the cursor moves past the inserted marker (6) instead of staying -1 |
| P5 | tasks/applyTaskOp | 152 | op type outside the TaskOp union (fixture mutation): the typed Rust TaskOp cannot represent it; TS falls through the switch as a no-op |
| P5 | tasks/applyTaskOp | 154 | auto value outside the 'merge' \| 'done' union (fixture mutation): the typed Rust TaskAuto cannot represent it |
| P5 | tasks/applyTaskOp | 210 | op type outside the TaskOp union (fixture mutation): the typed Rust TaskOp cannot represent it; TS falls through the switch as a no-op |
| P5 | tasks/applyTaskOp | 252 | op type outside the TaskOp union (fixture mutation): the typed Rust TaskOp cannot represent it; TS falls through the switch as a no-op |
| P5 | tasks/applyTaskOp | 277 | op type outside the TaskOp union (fixture mutation): the typed Rust TaskOp cannot represent it; TS falls through the switch as a no-op |
| P5 | tasks/cleanTaskTitle | 258 | slice(0, 300) cuts a surrogate pair: TS returns a lone surrogate, which a Rust String cannot hold (it becomes U+FFFD, as Node writes it to disk) |
| P5 | tasks/cleanTaskTitle | 259 | slice(0, 300) cuts a surrogate pair: TS returns a lone surrogate, which a Rust String cannot hold (it becomes U+FFFD, as Node writes it to disk) |
| P7 | changesModel/changeKey | 131 | mutated group 'unstagedunstaged' is outside the ChangeGroup union; the Rust enum (protocol wire type) rejects it at deserialization |
| P7 | changesModel/stageMode | 0 | mutated group '.untracked' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| P7 | changesModel/stageMode | 122 | mutated group 'un\nstaged' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| P7 | changesModel/stageMode | 143 | mutated group 'unstagéed' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| P7 | changesModel/groupOffset | 265 | mutated group 'unstage' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| P7 | changesModel/groupOffset | 268 | mutated group 'unstagedunstaged' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| P7 | changesModel/groupOffset | 309 | mutated group 'nstaged' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| P7 | changesModel/groupOffset | 311 | mutated group 'unsaged' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| P7 | changesModel/reselect | 326 | mutated group '' is outside the ChangeGroup union; unrepresentable in the Rust enum |
| U5 | settingsPane/render | 165 | TS writes the BEL in the candidate path of the unsanitized details line to the terminal as is; ratatui drops control characters from cells |
| U5 | settingsPane/render | 381 | TS writes the BEL in the candidate path of the unsanitized details line to the terminal as is; ratatui drops control characters from cells |
| U5 | settingsPane/render | 537 | TS writes the BEL in the candidate path of the unsanitized details line to the terminal as is; ratatui drops control characters from cells |
| U5 | settingsPane/render | 654 | TS writes the BEL in the candidate path of the unsanitized details line to the terminal as is; ratatui drops control characters from cells |
