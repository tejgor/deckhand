# Testing Deckhand

Tests describe the current Rust application's rules, presentation and compatibility guarantees. New tests do not
need a TypeScript counterpart, a port-unit ID or an export mapping.

## Running tests

Use the same entry point locally and in CI:

```sh
scripts/test.sh --no-fail-fast
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

The script builds the workspace before running each test once. This matters for `crates/e2e`, which launches the
built executable rather than linking the application. It uses `cargo-nextest` when installed and otherwise falls
back to `cargo test`. Arguments pass through to the selected runner: with nextest, a focused run can use
`scripts/test.sh -E 'test(settings)'`; with Cargo's runner, use `scripts/test.sh settings`.

For a unit test that does not launch the executable, use `cargo test -p deckhand-core <test-name>` or the equivalent
nextest filter directly. Rebuild the binary after application changes before running E2E directly. See
[`crates/e2e/README.md`](../crates/e2e/README.md) for the isolated daemon, fake agents and PTY harness.

E2E defaults to `target/debug/deckhand`. If Cargo uses a custom target directory, set `DECKHAND_TEST_BIN` to that
directory's debug executable when running the script, for example
`DECKHAND_TEST_BIN=/tmp/deckhand-target/debug/deckhand scripts/test.sh`.

## Choosing a test

| Test | What it protects | Expectations |
| --- | --- | --- |
| Named behavior tests beside the code | Rules such as layer precedence, pending trust, task transitions and deletion checks | Assert the result that matters, including edge cases and errors |
| UI snapshots | Layout, labels, hints and meaningful styles in representative states | Render Rust into a deterministic buffer and review changed snapshots |
| Compatibility tests | Reading older state, preserving user data, migrations and protocol contracts | Keep historical inputs frozen and assert the intended current result |
| E2E tests | The executable, daemon, workers, PTYs, persistence and recovery working together | Drive user workflows with private homes and fake agents |

Prefer named inputs and assertions that explain the rule. A global-only setting test should check its repository
cell is empty; it need not compare every other setting. Keep snapshots for distinct states such as an empty list,
an editor, a pending trust review or a narrow terminal. There is no fixed snapshot or case quota.

Generated properties complement examples for Unicode and offsets: compare conversions with independent standard
library results, check valid boundaries and bounds, and check movement progresses toward an endpoint. Explicit
examples still define intended word boundaries and invalid-offset behavior. Preserve minimized failures as
regression tests.

## Updating snapshots

First understand the failure and verify the behavior change with ordinary assertions. Update only the affected
tests, for example:

```sh
DECKHAND_BLESS=1 cargo nextest run -p deckhand-ui -E 'test(panes::settings::)'
git diff -- fixtures/ui
cargo nextest run -p deckhand-ui -E 'test(panes::settings::)'
```

`DECKHAND_BLESS` updates text snapshots and the remaining replay expectations. It cannot distinguish a fix from a
regression. Keep correctness rules as assertions, review every changed expectation, and rerun without the variable.
The replay encoding and update mechanism are documented in [`parity/README.md`](../parity/README.md).

## Existing corpora and compatibility

The port's sampled logic fixtures remain useful regression coverage. Keep low-churn corpora while they provide
value; when a module changes, replace redundant cases only after identifying their rules and edge cases. Settings
grid/render and help output now use named Rust tests instead of sampled output replays. Other modules can migrate
incrementally. New behavior does not require a new sampled fixture corpus.

`fixtures/state` contains historical saved-state inputs and reference results. Never bless it. Tests must keep old
inputs loading and preserve the documented data and migration guarantees; a future schema may have additional
fields in its current result. `fixtures/wire` protects documented message shapes and serialization, and
`fixtures/term` provides terminal-rendering conformance cases.

The Ink/Node vectors under `crates/*/tests/data` currently protect deliberate compatibility choices such as key
decoding, display width and string behavior. Changing those choices requires reviewing the relevant contract.
Their origin alone does not make every Node error phrase or incidental JSON key order a permanent API promise;
retain exact bytes where they affect a format, revision/hash, protocol or explicitly chosen behavior.

`parity/units`, TS generators, `scripts/verify-unit` and `docs/port` record how the port was checked. Existing
replays still read fixture metadata from those manifests. They are not requirements for new Rust tests; current
development uses the Cargo checks above.

## Isolation

Tests that touch files, Git or processes use temporary `HOME` and `DECKHAND_HOME` directories. Keep Git hermetic,
override version lookups with `DECKHAND_AGENT_LATEST`, and use fake agents. Never run tests against real Deckhand or
provider state, or start a real agent. E2E sandbox guards and process cleanup remain part of the test contract.
