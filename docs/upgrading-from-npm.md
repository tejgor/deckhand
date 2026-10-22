# Upgrading from Deckhand 2 (npm)

Deckhand 2 was a TypeScript app installed from npm (`@tejgor/deckhand`); Deckhand 3 is a native binary. Your sessions, notes, tasks and settings carry over: Deckhand 3 reads the files in `~/.deckhand` as version 2 left them. [Installing Deckhand](install.md#upgrading-from-deckhand-2-npm) has the short version (a release's notes point here with *Daemon restart: required when upgrading from the npm version*); this is the careful one, which keeps a copy of your state so you can go back. **It changes your real Deckhand: run it by hand, not through an agent or script.**

Before you start:
- **Run it from a plain terminal, not from inside a Deckhand session.** Step 2 stops the TypeScript daemon, which ends every agent session it hosts, including an agent you might be talking to.
- **`~/.deckhand/worktrees/` holds real git worktrees**, possibly including the checkout you build from. Never delete `~/.deckhand` as a whole; the rollback in step 6 restores only the state files.

1. **Back up the real state.** The persisted formats are unchanged, but keep a copy. The command skips the git worktrees, which can be large and are not state, and the live socket:

   ```sh
   rsync -a --exclude worktrees --exclude daemon.sock ~/.deckhand/ ~/.deckhand.pre-rust/
   ```

2. **Stop the running TypeScript daemon.** This ends every running session; make sure nothing in flight needs you first. Conversation references survive in `state.json`, so `s` resumes them later.

   ```sh
   kill "$(cat ~/.deckhand/daemon.pid)"       # the TypeScript daemon; wait until it exits
   pgrep -F ~/.deckhand/daemon.pid || echo stopped
   rm -f ~/.deckhand/daemon.pid ~/.deckhand/daemon.sock   # only if they remain after it exited
   ```

   Or leave it running until step 3 is done and run `deckhand restart` then: the native binary stops the TypeScript daemon the same way (after asking, since it ends the running sessions) and starts its own. Until either happens, the native `deckhand` refuses the old daemon with *daemon protocol mismatch … a daemon restart is required*.

3. **Install the native binary** (one of):

   ```sh
   brew install tejgor/tap/deckhand
   curl -fsSL https://github.com/tejgor/deckhand/releases/latest/download/deckhand-installer.sh | sh
   cargo install --path crates/deckhand                      # from the checkout
   cargo install --git https://github.com/tejgor/deckhand deckhand
   # or download the release archive, check it against sha256.sum, and install the binary on your PATH
   ```

   Homebrew puts the binary in its own `bin`; the installer in `~/.local/bin` (`$XDG_BIN_HOME` when set); `cargo install` in `~/.cargo/bin`. Keep only one of them: if you tried a `cargo install` build earlier and now use the installer, run `cargo uninstall deckhand`.

   Remove the old install so the shell cannot find it first (nvm's `bin` usually comes before the others on PATH):
   - **registry install:** `npm uninstall -g @tejgor/deckhand`
   - **`npm link` of a checkout:** check with `ls -l "$(npm root -g)" | grep deckhand`, then run `npm rm -g deckhand`. This removes only the symlinks; the checkout is untouched.

   Then run `hash -r` and check `which deckhand` (expect Homebrew's `bin`, `~/.local/bin` with the installer, or `~/.cargo/bin` with cargo) and `deckhand --version` (expect `deckhand 3.` followed by the latest release).

4. **Run it** from a repository: `deckhand`. Expected: a new daemon starts, your previous sessions appear as exited with their notes, tasks and ordering, and `s` resumes them.

5. **Doctor:** `deckhand doctor`. Expected: agents found, `✓ Daemon: <version> …, the same build as this deckhand`; if Codex hooks were set up for the TypeScript version, a warning about the stale `node ... cli.js hook` command. Fix it: `deckhand hooks codex`, merge the printed JSON into `~/.codex/hooks.json`, then `/hooks` in Codex to review.

6. **Roll back if needed.** This restores the state files only and leaves the worktrees alone:

   ```sh
   kill "$(cat ~/.deckhand/daemon.pid)"     # stop the native daemon; wait until it exits
   cd ~/.deckhand.pre-rust
   cp -a state.json config.json ui-state.json ~/.deckhand/
   cp -a notes/. ~/.deckhand/notes/          # likewise handoffs/ and notes-backups/ if you changed them
   npm i -g @tejgor/deckhand@2.0.0           # reinstall the TypeScript version (registry install), or:
   # cd <TS checkout> && npm run build && npm link      (if it was linked)
   ```

   Then report which step failed.
