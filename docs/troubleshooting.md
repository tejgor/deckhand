# Troubleshooting

Common problems and their fixes. Back to the [README](../README.md).

`deckhand doctor --check` is a good first step: it reports missing agents, the running daemon's version, stale Codex hooks and blocked notifications, and changes nothing.

## Deckhand can't find an agent

Check with `which claude` (or `codex`, `pi`).

Sessions inherit the `PATH` of the shell that started the daemon: the shell where you last ran `deckhand restart`, or ran `deckhand` while no daemon was running. After installing an agent or changing `PATH`, run `deckhand restart` from a shell that has the new `PATH`. This interrupts running sessions; `s` resumes each conversation.

## "daemon protocol mismatch … a daemon restart is required"

An older Deckhand's daemon is still running, usually just after an update. Run `deckhand restart`, then `deckhand` again. If that fails, stop the old daemon with `kill "$(cat ~/.deckhand/daemon.pid)"`. See [Updating](install.md#updating).

## "daemon … did not respond"

Look in `~/.deckhand/daemon.log`, then run `deckhand restart`.

## Codex hooks warning about `cli.js hook`

Codex still calls the old Node version of Deckhand. Run `deckhand hooks codex` and replace the stale entries in `$CODEX_HOME/hooks.json` (by default `~/.codex/hooks.json`); check `config.toml` in the same directory too. `deckhand doctor --check` lists what is left.

## `o` on the Git tab fails

Install [`lazygit`](https://github.com/jesseduffield/lazygit). The rest of the Git tab only needs `git`.

## The Dev tab is empty

Set a Dev command in Settings (`C`), then press `D` to start it. `d` only shows the tab.

## An agent's `deckhand` commands fail or ask first

- **Claude asks before each one**: check that `~/.deckhand/config.json` doesn't set `"allow_deckhand_commands": false`. The setting applies from a session's next launch.
- **Codex asks before each one**: expected. Codex has no launch option to allow a command, so its own approval settings apply.
- **An agent runs another `deckhand`**: ask it to run `command -v deckhand`. It should be `~/.deckhand/agent-plugin/bin/deckhand`, a link to the Deckhand that launched it. If your shell's startup files set `PATH` themselves, the agent's shell may find another copy first.

See [Deckhand commands for agents](features.md#deckhand-commands-for-agents).

## No notifications on macOS

Allow Deckhand in System Settings › Notifications › Deckhand. If it isn't listed there, switch Notifications off and on in Settings (`C`) to send a test. See [Allowing notifications on macOS](features.md#allowing-notifications-on-macos).
