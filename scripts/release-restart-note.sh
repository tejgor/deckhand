#!/bin/sh
# Prints the "Daemon restart" line of a release's notes: whether updating to <tag> needs the running daemon restarted.
#
#   scripts/release-restart-note.sh <tag>               the previous release is found among the v* tags
#   scripts/release-restart-note.sh <prev-tag> <tag>    explicit; an empty <prev-tag> means there is none
#
# A restart is required when PROTOCOL_VERSION (crates/protocol/src/lib.rs) differs between the two tags: a client
# refuses a daemon of another protocol. Without a previous release that has crates/protocol (none, or only the npm
# versions), the line points at the npm upgrade guide. Run from inside the repository; needs the tags fetched.
set -eu

PROTOCOL_FILE=crates/protocol/src/lib.rs

usage() {
    echo "usage: $0 [<prev-tag>] <tag>" >&2
    exit 2
}

# The newest stable v* tag older than $1 by version order (prereleases are skipped: users update from a release).
previous_tag() {
    git tag -l 'v*' --sort=-v:refname | grep -v -- - | awk -v tag="$1" 'found { print; exit } $0 == tag { found = 1 }'
}

# PROTOCOL_VERSION at a revision, or nothing when that revision has no crates/protocol.
protocol_at() {
    git show "$1:$PROTOCOL_FILE" 2>/dev/null |
        sed -n 's/^pub const PROTOCOL_VERSION: u32 = \([0-9][0-9]*\);.*/\1/p' |
        head -n 1
}

case $# in
    1)
        tag=$1
        git rev-parse -q --verify "refs/tags/$tag" >/dev/null || {
            echo "$0: no tag $tag" >&2
            exit 1
        }
        previous=$(previous_tag "$tag")
        ;;
    2)
        previous=$1
        tag=$2
        ;;
    *) usage ;;
esac

current=$(protocol_at "$tag")
if [ -z "$current" ]; then
    echo "$0: no PROTOCOL_VERSION in $PROTOCOL_FILE at $tag" >&2
    exit 1
fi
before=
if [ -n "$previous" ]; then
    before=$(protocol_at "$previous")
fi

if [ -z "$before" ]; then
    echo '**Daemon restart:** required when upgrading from the npm version — see docs/upgrading-from-npm.md.'
elif [ "$before" != "$current" ]; then
    echo "**Daemon restart:** required — protocol $before → $current. Run \`deckhand restart\` after updating (running sessions are interrupted; resume them with s)."
else
    echo '**Daemon restart:** not required — the running daemon stays compatible; `deckhand restart` when convenient picks up daemon fixes.'
fi
