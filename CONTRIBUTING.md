# Contributing to Deckhand

Bug reports, fixes and ideas are welcome.

## Before you start

- **Bugs**: open an issue with your version (`deckhand --version`), OS, agent and the steps that show it.
- **Small fixes**: open a pull request directly.
- **Larger changes or new features**: open an issue first, so we can agree on the approach before you spend time on it.
- **Security issues**: never in public. See [SECURITY.md](SECURITY.md).

## Set up

```sh
git clone https://github.com/tejgor/deckhand.git
cd deckhand
cargo build -p deckhand
scripts/deckhand-dev.sh --sandbox
```

The sandbox runs your build against its own state in `~/.deckhand-dev` and a disposable repository, beside your real Deckhand. Never point a development build at `~/.deckhand`: debug builds refuse it. See [docs/dev-build.md](docs/dev-build.md).

The promo videos live in their own private repository. Deckhand ignores `promo/`, where a maintainer clones it, and nothing in Deckhand needs it.

## Make your change

- Work on a branch in your fork.
- Add or update tests for behaviour you change. [docs/testing.md](docs/testing.md) explains behaviour tests, snapshots and fixtures. Tests use temporary homes and fake agents; they never start a real agent.
- If a recorded fixture or snapshot changes on purpose, follow [Changing behaviour](fixtures/cases/README.md#changing-behaviour-blessing). Never bless a failure you don't understand.
- Update the docs (`README.md`, `docs/`) when what users see or do changes.

Before you push, run the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
scripts/test.sh
```

## Pull requests

- Open the pull request against `main` and say what it changes and why.
- CI runs the checks above on Linux and macOS. For a first-time or outside contributor, the maintainer approves the workflow run first, so CI may wait a little before it starts.
- Every pull request needs the checks to pass and the maintainer's review. Resolve the review conversations before merging.
- Pull requests are squash-merged, so the title becomes the commit message on `main`. Write it like the project's history: `fix(ui): keep the selection after a reload`, `feat(tasks): …`, `docs: …`.
- Keep a pull request to one change. Two unrelated fixes are easier to review as two pull requests.

## Licence

Deckhand is released under the [MIT License](LICENSE). By contributing, you agree that your contributions are released under it too.
