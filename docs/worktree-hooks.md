# Worktree creation hooks

A repository script that creates the worktree for a new-worktree session, for layouts the `worktree` setting can't express. Back to the [README](../README.md).

For a new-worktree session, Deckhand creates or resolves a git worktree, then starts the agent inside it. The declarative [`worktree` setting](features.md#worktree-settings) covers most layouts: location, branch, shared dependency directories and env files. The worktree creation hook is the escape hatch for anything else:

```text
.claude/scripts/create-worktree.sh
```

Deckhand runs the hook only after you review and trust its exact contents, together with the repository's `deckhand.json` (see [Trust](features.md#trust-inline-review)). Otherwise it falls back to `git worktree add`.

Set `"worktree": {"hook": false}` to ignore the script entirely, or switch **Creation hook** off in Settings (`C`). It is then never run or reviewed. A repository's `deckhand.json` can switch the hook off even before it is trusted, since that only prevents execution.

## Contract

- **Input**: JSON on `stdin`: `name` (the sanitised worktree and session name) and `cwd` (the directory where `deckhand` was launched).
- **Job**: create or register a git worktree.
- **Output**: print the worktree's path to `stdout` as the final non-empty line. Send everything else, such as Git's own output, to `stderr`.
- **Exit status**: `0` on success. A non-zero exit fails the session's creation and shows the hook's `stderr`.
- **Environment**: `CLAUDE_PROJECT_DIR` is set to the launch directory, for compatibility with Claude-style hooks.
- **Working directory**: the root of the checkout Deckhand was launched from.

Deckhand enforces these limits:

- **Timeout**: the hook is stopped after 60 seconds, and creation fails with `create-worktree.sh timed out after 60s`.
- **Path**: the printed path must be absolute and must be a worktree Git has registered. Otherwise creation fails (`returned a non-absolute path`, or `created path is not registered as a git worktree`).
- **Lookup**: the script is looked up in the current checkout first, then in the main checkout. Only a regular file counts.
- **Execution**: Deckhand runs the reviewed bytes with `bash -c`, so a file swapped after review never runs. `$0` is the script path but `${BASH_SOURCE[0]}` is empty, so locate sibling files with `$0`.
- **Settings**: while a hook is active, it decides the location and the branch. `worktree.location`, `branchFrom` and `branchName` are ignored, and the new-session form can't choose a base branch.
- **Links**: `worktree.symlink` and `worktree.files` still apply after the hook runs, but only when the hook created a new worktree, not when it returned an existing one.

## Example

A minimal hook that reuses an existing worktree or creates one under the state directory:

```bash
#!/bin/bash
set -e

INPUT="$(cat)"
NAME="$(echo "$INPUT" | jq -r '.name // "worktree"')"
CWD="$(echo "$INPUT" | jq -r '.cwd // env.CLAUDE_PROJECT_DIR // env.PWD')"
DIR="${DECKHAND_HOME:-$HOME/.deckhand}/worktrees/$NAME"
START="$(git -C "$CWD" rev-parse HEAD)"

if [ -d "$DIR/.git" ] || git -C "$DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "$DIR"
  exit 0
fi

if [ -e "$DIR" ]; then
  echo "Path exists but is not a git worktree: $DIR" >&2
  exit 1
fi

if git -C "$CWD" show-ref --verify --quiet "refs/heads/$NAME"; then
  git -C "$CWD" worktree add "$DIR" "$NAME" >&2
else
  git -C "$CWD" worktree add -b "$NAME" "$DIR" "$START" >&2
fi

echo "$DIR"
```
