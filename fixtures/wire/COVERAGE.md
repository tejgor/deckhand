# Wire corpus coverage

Replayed by `crates/protocol/tests/wire_corpus.rs` (`cargo test -p deckhand-protocol`).

## Files

- `corpus.jsonl`: captured from the TypeScript version's test suite, where the daemon, client and both worker kinds
  logged every message they sent or received.
  The reduction keeps one message per distinct shape (key order and value kinds, recursively), the smallest one,
  at most 24 per (dir, worker, type, request type), sorted. Machine-specific path prefixes are rewritten
  (`/tmp`, `/private/tmp`, `/home/user`, `/src/deckhand`).
  When a message changes shape on purpose, edit its lines by hand.
- `extra.jsonl`: hand-written messages for the variants the suite never sends, derived from daemon.ts,
  sessionWorker.ts and attach.ts. Sessions and pane records are borrowed from the corpus and merged as the TS
  spreads merge them.
- `records/`: `state.json`, `config.json` and `ui-state.json` as storage.ts and uiState.ts write them (recorded against a temp
  `DECKHAND_HOME`), plus TS's normalised `AppConfig` (`app-config.normalized.json`) and
  `normalizeUiState` cases (`ui-state.normalize.jsonl`).

Each corpus line is `{dir, worker?, req?, msg}`:

- `dir` is `c2d`, `d2c`, `d2w` or `w2d`.
- `worker` (`session` | `workspace`) names the IPC peer.
- `req` is the type of the request a response answers.

## What the test checks

- Every line deserialises into its typed enum (`Request`, `ServerMessage`, `WorkerCommand`,
  `WorkspaceWorkerCommand`, `WorkerMessage`). It must serialise back to the identical JSON text, key order included.
- Successful responses with a `req` also type their `data` (`ResponseData::decode`, `WorkerResponseData::decode`)
  and must round-trip identically.
- Requests (`c2d`) are written by many producers: client.ts, attach.ts, cli.ts and tests that build their own
  literals. Their key order is the producer's, so they are checked through `Ordered<Request>`, which keeps the order
  it read. The daemon's `kill` to a session worker likewise has two producers.
- Every other message is checked with plain typed structs, so their field order must equal the TS construction
  order. Records whose TS key order depends on history are `Ordered<T>` fields: sessions, pane records, project
  configs, worktree candidates, merge results and attach results.
- `the_corpus_covers_every_variant` fails if any request, event, worker command or worker message variant, or any
  data-carrying response type, has no line.
- The records round-trip byte-for-byte through `records::to_ts_file_text`. `AppConfig::from_raw` and
  `UiState::normalize` must match TS's normalised output.

## Coverage

Counts of lines per variant: captured (corpus) and hand-written (extra). `response (failure)` lines are `ok: false`.

| dir | worker | type | corpus | extra |
|-----|--------|------|-------:|------:|
| c2d | - | `action-detach` | 0 | 1 |
| c2d | - | `action-input` | 0 | 1 |
| c2d | - | `action-resize` | 0 | 1 |
| c2d | - | `agent-hook` | 11 | 0 |
| c2d | - | `agent-versions` | 3 | 0 |
| c2d | - | `archive-session` | 2 | 0 |
| c2d | - | `attach-action` | 0 | 1 |
| c2d | - | `attach-dev` | 0 | 1 |
| c2d | - | `attach-git` | 0 | 1 |
| c2d | - | `attach-terminal` | 2 | 1 |
| c2d | - | `attach` | 0 | 1 |
| c2d | - | `cancel-start` | 1 | 0 |
| c2d | - | `change-stage` | 3 | 0 |
| c2d | - | `changes-diff` | 2 | 0 |
| c2d | - | `create-pr` | 2 | 0 |
| c2d | - | `create` | 14 | 0 |
| c2d | - | `delete-worktree` | 8 | 0 |
| c2d | - | `detach` | 0 | 1 |
| c2d | - | `dev-detach` | 0 | 1 |
| c2d | - | `dev-input` | 0 | 1 |
| c2d | - | `dev-resize` | 0 | 1 |
| c2d | - | `export-handoff` | 1 | 0 |
| c2d | - | `git-detach` | 0 | 1 |
| c2d | - | `git-input` | 0 | 1 |
| c2d | - | `git-resize` | 0 | 1 |
| c2d | - | `global-link-candidates` | 0 | 1 |
| c2d | - | `input` | 1 | 1 |
| c2d | - | `inspect-cleanup` | 3 | 0 |
| c2d | - | `inspect-worktree` | 2 | 0 |
| c2d | - | `kill` | 7 | 0 |
| c2d | - | `list-branches` | 2 | 0 |
| c2d | - | `list-worktrees` | 1 | 0 |
| c2d | - | `list` | 1 | 0 |
| c2d | - | `mark-session-merged` | 1 | 0 |
| c2d | - | `merge-preview` | 2 | 0 |
| c2d | - | `merge-worktree` | 4 | 0 |
| c2d | - | `open-note` | 1 | 0 |
| c2d | - | `open-tasks` | 0 | 1 |
| c2d | - | `ping` | 1 | 0 |
| c2d | - | `project-info` | 2 | 0 |
| c2d | - | `promote-note-item` | 2 | 0 |
| c2d | - | `remove` | 3 | 0 |
| c2d | - | `reorder-session` | 0 | 1 |
| c2d | - | `resize` | 0 | 1 |
| c2d | - | `resolve-merge` | 2 | 0 |
| c2d | - | `restart` | 2 | 0 |
| c2d | - | `return-task-to-note` | 2 | 0 |
| c2d | - | `run-action` | 2 | 0 |
| c2d | - | `save-config` | 4 | 0 |
| c2d | - | `save-note` | 3 | 0 |
| c2d | - | `send-open-items` | 2 | 0 |
| c2d | - | `set-reply-read` | 0 | 1 |
| c2d | - | `set-session-done` | 2 | 0 |
| c2d | - | `settings-info` | 2 | 0 |
| c2d | - | `shutdown` | 1 | 0 |
| c2d | - | `start-dev` | 2 | 0 |
| c2d | - | `stop-action` | 2 | 0 |
| c2d | - | `stop-dev` | 1 | 0 |
| c2d | - | `subscribe` | 2 | 0 |
| c2d | - | `task-op` | 10 | 0 |
| c2d | - | `task-to-note` | 1 | 0 |
| c2d | - | `terminal-detach` | 1 | 1 |
| c2d | - | `terminal-input` | 1 | 1 |
| c2d | - | `terminal-resize` | 0 | 1 |
| c2d | - | `trust-project` | 2 | 0 |
| c2d | - | `update-agent` | 1 | 0 |
| c2d | - | `watch-action` | 2 | 0 |
| c2d | - | `watch-changes` | 3 | 0 |
| c2d | - | `watch-dev` | 3 | 0 |
| c2d | - | `watch-git` | 1 | 0 |
| c2d | - | `watch-preview` | 2 | 0 |
| c2d | - | `watch-tasks` | 2 | 0 |
| c2d | - | `watch-terminal` | 2 | 0 |
| c2d | - | `workspace-summary` | 2 | 0 |
| c2d | - | `worktree-candidate-sizes` | 2 | 0 |
| c2d | - | `worktree-candidates` | 2 | 0 |
| c2d | - | `worktree-overview` | 2 | 0 |
| d2c | - | `action-attached` | 0 | 1 |
| d2c | - | `action-detached` | 0 | 1 |
| d2c | - | `action-output` | 0 | 1 |
| d2c | - | `action-updated` | 3 | 0 |
| d2c | - | `agent-versions-updated` | 6 | 0 |
| d2c | - | `attached` | 0 | 1 |
| d2c | - | `changes-updated` | 2 | 0 |
| d2c | - | `detached` | 0 | 1 |
| d2c | - | `dev-attached` | 0 | 1 |
| d2c | - | `dev-detached` | 0 | 1 |
| d2c | - | `dev-output` | 0 | 1 |
| d2c | - | `dev-updated` | 2 | 0 |
| d2c | - | `git-attached` | 0 | 1 |
| d2c | - | `git-detached` | 0 | 1 |
| d2c | - | `git-output` | 0 | 1 |
| d2c | - | `git-updated` | 0 | 2 |
| d2c | - | `output` | 0 | 1 |
| d2c | - | `preview-updated` | 1 | 0 |
| d2c | - | `session-removed` | 1 | 0 |
| d2c | - | `session-updated` | 24 | 0 |
| d2c | - | `tasks-updated` | 1 | 0 |
| d2c | - | `terminal-attached` | 1 | 0 |
| d2c | - | `terminal-detached` | 1 | 0 |
| d2c | - | `terminal-output` | 1 | 0 |
| d2c | - | `terminal-updated` | 1 | 0 |
| d2c | - | response (failure) | 24 | 1 |
| d2c | - | response `agent-hook` | 1 | 0 |
| d2c | - | response `agent-versions` | 3 | 0 |
| d2c | - | response `archive-session` | 2 | 0 |
| d2c | - | response `attach-action` | 0 | 1 |
| d2c | - | response `attach-dev` | 0 | 1 |
| d2c | - | response `attach-git` | 0 | 1 |
| d2c | - | response `attach-terminal` | 1 | 0 |
| d2c | - | response `attach` | 0 | 1 |
| d2c | - | response `cancel-start` | 1 | 0 |
| d2c | - | response `change-stage` | 1 | 0 |
| d2c | - | response `changes-diff` | 1 | 0 |
| d2c | - | response `create-pr` | 1 | 0 |
| d2c | - | response `create` | 5 | 0 |
| d2c | - | response `delete-worktree` | 4 | 0 |
| d2c | - | response `export-handoff` | 1 | 0 |
| d2c | - | response `global-link-candidates` | 0 | 2 |
| d2c | - | response `inspect-cleanup` | 2 | 0 |
| d2c | - | response `inspect-worktree` | 3 | 0 |
| d2c | - | response `kill` | 1 | 0 |
| d2c | - | response `list-branches` | 1 | 0 |
| d2c | - | response `list-worktrees` | 1 | 0 |
| d2c | - | response `list` | 24 | 0 |
| d2c | - | response `mark-session-merged` | 5 | 0 |
| d2c | - | response `merge-preview` | 4 | 0 |
| d2c | - | response `merge-worktree` | 5 | 0 |
| d2c | - | response `open-note` | 1 | 0 |
| d2c | - | response `open-tasks` | 0 | 1 |
| d2c | - | response `ping` | 1 | 0 |
| d2c | - | response `project-info` | 13 | 0 |
| d2c | - | response `promote-note-item` | 2 | 0 |
| d2c | - | response `remove` | 1 | 0 |
| d2c | - | response `reorder-session` | 0 | 1 |
| d2c | - | response `resolve-merge` | 3 | 0 |
| d2c | - | response `restart` | 9 | 0 |
| d2c | - | response `return-task-to-note` | 1 | 0 |
| d2c | - | response `run-action` | 1 | 0 |
| d2c | - | response `save-config` | 2 | 0 |
| d2c | - | response `save-note` | 6 | 0 |
| d2c | - | response `send-open-items` | 2 | 0 |
| d2c | - | response `set-reply-read` | 0 | 1 |
| d2c | - | response `set-session-done` | 9 | 0 |
| d2c | - | response `settings-info` | 10 | 0 |
| d2c | - | response `shutdown` | 1 | 0 |
| d2c | - | response `start-dev` | 1 | 0 |
| d2c | - | response `stop-action` | 1 | 0 |
| d2c | - | response `stop-dev` | 1 | 0 |
| d2c | - | response `subscribe` | 5 | 0 |
| d2c | - | response `task-op` | 1 | 0 |
| d2c | - | response `task-to-note` | 1 | 0 |
| d2c | - | response `trust-project` | 4 | 0 |
| d2c | - | response `update-agent` | 1 | 0 |
| d2c | - | response `watch-action` | 4 | 0 |
| d2c | - | response `watch-changes` | 4 | 0 |
| d2c | - | response `watch-dev` | 5 | 0 |
| d2c | - | response `watch-git` | 1 | 0 |
| d2c | - | response `watch-preview` | 2 | 0 |
| d2c | - | response `watch-tasks` | 1 | 0 |
| d2c | - | response `watch-terminal` | 2 | 0 |
| d2c | - | response `workspace-summary` | 2 | 0 |
| d2c | - | response `worktree-candidate-sizes` | 1 | 0 |
| d2c | - | response `worktree-candidates` | 1 | 0 |
| d2c | - | response `worktree-overview` | 5 | 0 |
| d2w | session | `attach` | 0 | 1 |
| d2w | session | `detach` | 0 | 1 |
| d2w | session | `input` | 1 | 0 |
| d2w | session | `kill` | 2 | 0 |
| d2w | session | `resize` | 0 | 1 |
| d2w | session | `snapshot` | 0 | 1 |
| d2w | session | `start` | 22 | 0 |
| d2w | workspace | `attach` | 1 | 0 |
| d2w | workspace | `detach` | 1 | 0 |
| d2w | workspace | `idle` | 1 | 0 |
| d2w | workspace | `input` | 1 | 0 |
| d2w | workspace | `resize` | 0 | 1 |
| d2w | workspace | `shutdown` | 1 | 0 |
| d2w | workspace | `snapshot` | 1 | 0 |
| d2w | workspace | `start-action` | 1 | 0 |
| d2w | workspace | `start-dev` | 1 | 0 |
| d2w | workspace | `start` | 1 | 0 |
| d2w | workspace | `stop-action` | 1 | 0 |
| d2w | workspace | `stop-dev` | 1 | 0 |
| w2d | session | `agent-status` | 1 | 0 |
| w2d | session | `exit` | 1 | 0 |
| w2d | session | `hello` | 0 | 1 |
| w2d | session | `output` | 0 | 1 |
| w2d | session | `preview-updated` | 1 | 0 |
| w2d | session | `running` | 1 | 0 |
| w2d | session | response (failure) | 0 | 1 |
| w2d | session | response `attach` | 0 | 1 |
| w2d | session | response `kill` | 1 | 0 |
| w2d | session | response `snapshot` | 0 | 2 |
| w2d | session | response `start` | 22 | 0 |
| w2d | workspace | `action-updated` | 3 | 0 |
| w2d | workspace | `dev-updated` | 3 | 0 |
| w2d | workspace | `git-updated` | 1 | 0 |
| w2d | workspace | `hello` | 0 | 1 |
| w2d | workspace | `output` | 1 | 0 |
| w2d | workspace | `terminal-updated` | 1 | 0 |
| w2d | workspace | response (failure) | 1 | 0 |
| w2d | workspace | response `attach` | 1 | 1 |
| w2d | workspace | response `idle` | 1 | 0 |
| w2d | workspace | response `shutdown` | 1 | 0 |
| w2d | workspace | response `snapshot` | 4 | 0 |
| w2d | workspace | response `start-action` | 1 | 0 |
| w2d | workspace | response `start-dev` | 1 | 0 |
| w2d | workspace | response `start` | 1 | 0 |
| w2d | workspace | response `stop-action` | 1 | 0 |
| w2d | workspace | response `stop-dev` | 1 | 0 |
