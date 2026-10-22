# Installing Deckhand

Every way to install, update and remove Deckhand. Back to the [README](../README.md).

## Requirements

- macOS or Linux, on x86_64 or aarch64. The Linux binaries need glibc 2.35 or newer.
- Windows: run Deckhand inside [WSL2](https://learn.microsoft.com/windows/wsl/install), using the Linux binary.
- `git`, and at least one agent on `PATH`: `claude`, `codex` or `pi`.

Optional:

- [`lazygit`](https://github.com/jesseduffield/lazygit) for `o` on the Git tab.
- Cursor or VS Code for `E`, which opens a worktree, note or file in your editor. Other editors aren't supported.
- [`gh`](https://cli.github.com), signed in, to open pull requests.
- `npm` for the agent update check on the Agents screen (`U`).
- On Linux, `notify-send` for desktop notifications.

## Install

Pick one method and stick to it, so only one `deckhand` is on your `PATH`.

### Homebrew

On macOS and Linux:

```sh
brew install tejgor/tap/deckhand
```

### Installer script

On macOS and Linux, with no Rust needed:

```sh
curl -fsSL https://github.com/tejgor/deckhand/releases/latest/download/deckhand-installer.sh | sh
```

It downloads the latest release's binary for your platform into `~/.local/bin` (`$XDG_BIN_HOME` when set, else `$XDG_DATA_HOME/../bin` when that is set), and adds the directory to your `PATH` if it isn't there yet.

### Release archive

Download the archive for your platform and `sha256.sum` from the [releases page](https://github.com/tejgor/deckhand/releases). In the folder you downloaded them to, check the archive and put the binary on your `PATH` (here `~/.local/bin`, which must be on your `PATH`):

```sh
TARGET=aarch64-apple-darwin       # or x86_64-apple-darwin, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu
shasum -a 256 -c sha256.sum --ignore-missing
tar -xJf "deckhand-$TARGET.tar.xz"
mkdir -p ~/.local/bin && install -m 755 "deckhand-$TARGET/deckhand" ~/.local/bin/deckhand
```

If macOS refuses to open it, remove the download quarantine: `xattr -d com.apple.quarantine ~/.local/bin/deckhand`.

### Cargo

With Rust 1.88 or newer:

```sh
cargo install --git https://github.com/tejgor/deckhand deckhand
```

This puts the binary in `~/.cargo/bin`.

## Set up agents

`deckhand setup` checks for Claude Code, Codex and Pi and offers to install the missing ones with their official installers. `deckhand setup --check` only reports.

## Updating

Update the way you installed:

| Installed with | Update with |
| --- | --- |
| Homebrew | `brew upgrade deckhand` |
| Installer script | Run the installer again |
| Release archive | Download and install the new archive |
| Cargo | Add `--force` to the `cargo install` command |

Then check the **Daemon restart** line in the [release notes](https://github.com/tejgor/deckhand/releases):

- **Required**: the new version speaks a different protocol to the daemon, so `deckhand` refuses the running daemon until you run `deckhand restart`.
- **Not required**: Deckhand keeps using the running daemon and tells you once, at the bottom of the screen (`Daemon 3.0.0 is still running (this is 3.0.1) · restart when convenient: deckhand restart`). Run `deckhand restart` when it suits you, to pick up daemon fixes.

`deckhand restart` stops the daemon and starts the new version's. This interrupts running sessions, and `s` resumes each conversation. It asks first when sessions are running; `--yes` skips the question, and is required when there is no terminal to ask on. `deckhand doctor --check` shows which version the running daemon is.

## Upgrading from Deckhand 2 (npm)

Your sessions, notes and settings carry over: the files in `~/.deckhand` are unchanged.

1. Remove the npm install (`npm uninstall -g @tejgor/deckhand`) and install Deckhand 3 as above. Check with `deckhand --version`.
2. Restart the daemon: `deckhand restart` stops the old one and starts the new one. This ends its running sessions; `s` resumes each conversation later.
3. If you use Codex with agent signals, regenerate its hooks: `deckhand doctor --check` tells you if they still call the old version.

[Upgrading from npm](upgrading-from-npm.md) is the careful version, with a backup and a way back.

## Uninstall

Stop the daemon first, which ends any running sessions:

```sh
kill "$(cat ~/.deckhand/daemon.pid)"
```

Then remove the binary:

```sh
brew uninstall deckhand       # if installed with Homebrew
cargo uninstall deckhand      # if installed with cargo
rm "$(command -v deckhand)"   # if installed with the installer script or from a release archive
```

To remove all local state, delete `~/.deckhand`.

> [!WARNING]
> `~/.deckhand/worktrees/` holds git worktrees, which may contain uncommitted work. Remove them first, from the Worktrees screen (`W`) or with `git worktree remove`, so git's records stay consistent.
