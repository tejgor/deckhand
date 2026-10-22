# Security policy

## Supported versions

Security fixes go into the latest release of Deckhand 3 (the Rust version). Deckhand 2, the TypeScript version on npm, no longer gets fixes: upgrade as described in the [README](README.md#upgrading-from-v2-npm).

## Reporting a vulnerability

Please report it privately, not in a public issue, pull request or discussion.

Use GitHub's private reporting: the repository's **Security** tab → **Report a vulnerability**, or go straight to [the new advisory form](https://github.com/tejgor/deckhand/security/advisories/new). Only the maintainer sees it.

Include what you can of:

- the version (`deckhand --version`) and your OS
- what an attacker can do, and what they need first (a local account, a crafted repository, a malicious agent, …)
- the steps to reproduce it

You'll get a reply in the advisory. Once a fix is released, the advisory is published, crediting you if you want.

## What counts

Deckhand runs coding agents and commands on your machine, so these matter most:

- a repository's `deckhand.json` (Dev command, setup command, actions) running before you trusted its exact contents
- another local user, or a process outside your sessions, reaching the daemon's socket or passing an agent's hook token
- Deckhand reading or changing files outside your repository, its worktrees and its state directory (`~/.deckhand`) without being asked
- deleting worktrees or branches that hold uncommitted work or unmerged commits without the confirmation the docs describe

What an agent does with the permissions you give it is up to the agent and its own approvals, not Deckhand.
