# Test the isolated dev build

The feature branch can run beside your installed Deckhand without replacing its binary or reconnecting to its daemon. **Do not use `npm link`, reinstall globally, or stop the production daemon.**

## Start in a disposable repository

From the feature checkout:

```sh
cd /path/to/deckhand-no-brainers
npm start
```

This builds this checkout's `dist/` and opens a dependency-free Git sandbox at `~/.deckhand-dev/sandbox`. The UI displays **deckhand · DEV (isolated)**. With a fresh dev namespace there are no existing sessions.

| Production | Dev build |
| --- | --- |
| Installed `deckhand` | This checkout's `dist/cli.js` |
| `~/.deckhand/` | `~/.deckhand-dev/` |
| Production socket/PID/logs/state/trust/UI preferences | Separate socket/PID/logs/state/trust/UI preferences |
| `~/.deckhand/worktrees/` | `~/.deckhand-dev/worktrees/` (Git fallback) |

The wrapper sets `DECKHAND_HOME` to the dev namespace for the processes it starts. An optional **`DECKHAND_DEV_HOME`** selects another dev namespace. It must be separate from `~/.deckhand` and from any `DECKHAND_HOME` your shell already sets for stable Deckhand: the same path, an ancestor or descendant, or a symlink alias of either is rejected.

To run directly from TypeScript instead, use `node scripts/deckhand-dev.mjs --source --sandbox`.

`npm run dev` and `npm run daemon` are **not isolated**; use the commands here for this preview.

## Status, stop, and rebuild

```sh
npm run status   # is the dev daemon running?
npm stop         # stops ONLY the dev daemon and its agents/auxiliary PTYs
npm restart      # stop, rebuild and reopen (use after daemon/worker code changes)
```

`q` closes only the UI. Reopening reconnects to the dev daemon. Source/build changes do not hot-reload an already-running daemon: stop the **dev** daemon before testing updated daemon/worker code. Dev status/stop never start a daemon, and refuse to control a daemon that is not a dev-channel daemon for this exact namespace (compared by real path).

## Manual smoke checklist

1. Press **?** for the scrollable help/workflow guide; Page keys or j/k scroll and Escape returns. Press **C** for Settings: a grid with a **Global (all repos)** and a **This repo** column; the sandbox's untrusted `deckhand.json` values sit in the This repo column with **⚠ needs trust** while ● marks what applies now (built-ins in italics, e.g. *dev (built-in)*). The cursor starts on This repo: change **Default agent** with Enter (saves to the sandbox's `deckhand.json`; the status says *review required* since the file is unreviewed), press **←** and set **Dev command** with Enter (saves to `defaults` in `~/.deckhand-dev/config.json`); the column stays put while ↑↓ move, and the one-line box under the grid names the cell's layer and file. **Linked items** lists untracked/ignored entries with sizes; Space toggles, Enter saves that column. **e** opens the selected column's raw JSON (arrows/Home/End move, Ctrl+A selects all, Ctrl+F formats, Ctrl+S validates/saves, Esc or Ctrl+C returns); invalid drafts must leave the file untouched. **T** trusts inline and returns to Settings; an edit made after that says *still trusted*, and **d** then starts the repository Dev command without asking — until you edit `deckhand.json` outside Deckhand, which is reviewed again. Saving never runs anything.
2. Press **n**: the agent picker opens immediately, with the sandbox's default workspace (**new worktree**) preselected even before trust. Select an installed agent and enter a unique name. If the sandbox's `deckhand.json` is still untrusted and its setup, hook or worktree settings would be used, confirming shows its exact contents: **Enter** trusts it and creates the session, **s** creates it with global settings only, **Esc** goes back. **T** in Settings (**C**) opens the same review at any time. Agent authentication/billing is real; no fake responses are used in the manual UI.
3. Press **e**, choose `test`, and Enter. The Terminal tab should switch to the action and show the sandbox's passing `node --test` output, with `test ✓ exit 0` in its header; `v` switches back to the shell. `d` then `d` starts its example server on port 4319 (running `e` → `test` again meanwhile works: actions don't share Dev); stop it with `d` again.
4. Press **i** for local Git information. Change a file in this disposable worktree and reopen the overview to see its dirty count. **P** explicitly asks `gh` for a PR; this local sandbox has no PR, so an unavailable message is normal. **g** opens lazygit if installed.
5. Press **a**, then **o**, and add notes. Escape finishes editing. Press **H** to export/open a handoff; inspect/edit the document before pressing **F** to create a clean child with that document as initial context. No conversation is forked by F.
6. Press **A** to archive a session, **f** to switch to archived/all, and **/** to search its title or notes. Quit/reopen to verify archive, search/filter, selected tab, sidebar size, and tree organization persist. Archive does **not** stop a running agent.
7. On a worktree session, press **x** and choose deletion. Modified, untracked or valuable ignored files, commits that deleting the branch would lose, or Git state that cannot be checked should require a **DELETE** confirmation. The main checkout and worktrees in use by another session never offer deletion. Escape cancels. Force kill (**X**) alone never authorizes data loss. Use the sandbox for destructive tests.
8. To test setup, add a `setupCommand` to the sandbox config and create another new worktree; **n** asks you to review the changed bytes first. Setup output appears while starting. **x** cancels setup without deleting the worktree; **s** retries it. A Git/custom-hook preparation already in flight can finish after cancellation, but the agent must not launch; its resulting worktree is retained in the session record.
9. Test lifecycle hooks/notifications using the opt-in instructions in [the feature guide](no-brainers.md#lifecycle-signals-and-notifications). Without supported/configured hooks, attention remains unknown/activity-based—not a claim that the task succeeded.
10. Exit a native agent normally, then **s** to resume its known identity or **S** for a fresh conversation. Codex without a captured native ID must ask for S, not silently open a blank conversation. `npm stop` ends only the test daemon.

## Open another repository deliberately

After building, run the wrapper by absolute path from that repository:

```sh
cd /path/to/a/disposable-repository
node /path/to/deckhand-no-brainers/scripts/deckhand-dev.mjs
```

**Isolation is not a filesystem or provider sandbox.** Choosing a real repository/no-worktree workspace can modify real files. Trusted custom scripts can write outside the dev directory; ensure custom creation hooks respect `DECKHAND_HOME`. Agents still use their normal credentials, native conversation stores, and subscriptions. Dev Codex launches require the advertised `--no-daemon` capability (even with lifecycle hooks disabled) rather than reusing a production native Codex daemon; unsupported versions are refused. Deckhand generates separate session identities; it does not copy or resume production Deckhand sessions. Prefer the default sandbox, avoid `deckhand setup` (which installs real global agent tools), and do not delete registered worktrees with `rm -rf`.

## Automated checks

```sh
npm test
```

Tests build this checkout and use temporary Git repositories, private state directories and fake agent binaries; they never touch `~/.deckhand` or `~/.deckhand-dev`. They cover config parsing, effective-settings merging, editing and trust; setup, actions and cleanup safeguards in a real daemon; lifecycle callbacks and resume; storage; a real-terminal run of the UI (inline review, raw key handling, persistence); and the dev launcher's isolation. They do not run real agent conversations, so provider hook behaviour in your installed CLI still needs the manual check above.
