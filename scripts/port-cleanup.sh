#!/bin/sh
# Removes what the Rust port's agent workflows left behind. Dry run by default; pass --yes to act.
#
#   scripts/port-cleanup.sh                     # show what would be removed
#   scripts/port-cleanup.sh --yes               # remove it (fast: moves to a trash dir, purges in the background)
#   scripts/port-cleanup.sh --yes --wait        # same, but purge in the foreground
#   scripts/port-cleanup.sh --yes --dev-homes   # also ~/.deckhand-rust-dev and ~/.deckhand-rust-copy
#   scripts/port-cleanup.sh --yes --force       # also worktrees/branches whose commits are not merged into rust-port
#
# Safe to re-run (e.g. after an interrupted run). Never touches: ~/.deckhand, the rust-port branch/checkout, main, your
# other worktrees and branches, the live daemon and its workers, or a dev home whose daemon is still running.
set -u

YES=0 DEV_HOMES=0 FORCE=0 WAIT=0
for arg in "$@"; do
	case $arg in
		--yes) YES=1 ;;
		--dev-homes) DEV_HOMES=1 ;;
		--force) FORCE=1 ;;
		--wait) WAIT=1 ;;
		-h|--help) sed -n '2,12p' "$0"; exit 0 ;;
		*) echo "unknown option: $arg" >&2; exit 2 ;;
	esac
done

here=$(cd "$(dirname "$0")/.." && pwd -P)
git() { command git -C "$here" "$@"; }
common=$(cd "$here" && cd "$(command git rev-parse --git-common-dir)" && pwd -P)
main_repo=$(dirname "$common")
wt_root=$main_repo/.claude/worktrees
tmp=${TMPDIR:-/tmp}; tmp=${tmp%/}
real_home=$(cd ~ && pwd -P)
stamp=$(date +%Y%m%d-%H%M%S)
trashes=""   # trash dirs created this run (one per filesystem root we moved from)
count=0

say() { printf '%s\n' "$*"; }
skip() { say "  skip     $*"; }
plan() { say "  would remove  $*"; count=$((count + 1)); }

# trash <path> <trash-dir>: rename into the trash (instant on the same filesystem); fall back to rm -rf.
trash() {
	src=$1 dir=$2
	if [ "$YES" = 0 ]; then plan "$src"; return; fi
	mkdir -p "$dir" 2>/dev/null
	case " $trashes " in *" $dir "*) ;; *) trashes="$trashes $dir" ;; esac
	if mv "$src" "$dir/$(basename "$src").$$.$count" 2>/dev/null || rm -rf "$src" 2>/dev/null; then
		count=$((count + 1))
	else
		say "  FAILED   $src"
	fi
}

git rev-parse --verify -q rust-port >/dev/null || { echo "rust-port branch not found from $here" >&2; exit 1; }
[ "$YES" = 1 ] || say "(dry run — pass --yes to remove)"

say "== 1. Leftover test daemons and workers (temp HOME or DECKHAND_HOME only)"
for pid in $(pgrep -f 'deckhand|fake_agent' 2>/dev/null); do
	[ "$pid" = $$ ] && continue
	line=$(ps -wwE -o command= -p "$pid" 2>/dev/null) || continue
	case $line in *deckhand*|*fake_agent*) ;; *) continue ;; esac
	case $line in *port-cleanup*|*Cursor*|*tsserver*|*claude*) continue ;; esac
	dh=$(printf '%s\n' "$line" | grep -oE 'DECKHAND_HOME=[^ ]+' | head -1 | cut -d= -f2-)
	h=$(printf '%s\n' "$line" | grep -oE '(^| )HOME=[^ ]+' | head -1 | sed 's/^ //' | cut -d= -f2-)
	where=${dh:-$h}
	if [ -z "$where" ]; then
		# Node retitles its process (process.title), hiding argv/env: fall back to the files it has open.
		files=$(lsof -p "$pid" -Fn 2>/dev/null | sed -n 's/^n//p')
		printf '%s\n' "$files" | grep -qE "^$real_home/\.deckhand(/|$)|^$real_home/\.deckhand-rust-" && continue
		where=$(printf '%s\n' "$files" | grep -E '/(daemon\.(log|sock|pid)|state\.json)' | grep -E '^(/tmp|/private/tmp|/var/folders|/private/var/folders)/' | head -1)
	fi
	case $where in
		/tmp/*|/private/tmp/*|/var/folders/*|/private/var/folders/*|"$tmp"/*)
			name=$(ps -o comm= -p "$pid" | sed 's#.*/##')
			if [ "$YES" = 1 ]; then kill "$pid" 2>/dev/null; say "  stopped  process $pid ($name, $where)"
			else plan "process $pid ($name, $where)"; fi ;;
	esac
done
[ "$YES" = 1 ] && sleep 1

say "== 2. Workflow worktrees under $wt_root"
wt_trash=$main_repo/.claude/.port-trash-$stamp
registered=$(git worktree list --porcelain | awk '/^worktree /{print substr($0, 10)}')
n_wt=0 n_skip=0
for wt in "$wt_root"/wf_*; do
	[ -e "$wt" ] || continue
	if printf '%s\n' "$registered" | grep -qxF "$wt" && [ "$FORCE" = 0 ]; then
		head=$(command git -C "$wt" rev-parse -q --verify HEAD 2>/dev/null || echo "")
		if [ -n "$head" ] && ! git merge-base --is-ancestor "$head" rust-port 2>/dev/null; then
			skip "$wt (HEAD $(printf %.9s "$head") not merged into rust-port; --force to remove)"; n_skip=$((n_skip + 1)); continue
		fi
		if [ -n "$(command git -C "$wt" status --porcelain --untracked-files=no 2>/dev/null | head -1)" ]; then
			skip "$wt (uncommitted changes; --force to remove)"; n_skip=$((n_skip + 1)); continue
		fi
	fi
	if [ "$YES" = 1 ]; then trash "$wt" "$wt_trash"; else count=$((count + 1)); fi
	n_wt=$((n_wt + 1))
done
say "  $( [ "$YES" = 1 ] && echo moved || echo 'would remove') $n_wt worktree dirs ($n_skip kept)"
# Forget worktrees whose directories are gone (moved above, or deleted by an earlier interrupted run).
[ "$YES" = 1 ] && git worktree prune

say "== 3. Port branches merged into rust-port"
merged=$(git branch --format='%(refname:short)' --merged rust-port)
all=$(git branch --format='%(refname:short)' | grep -E '^(w0|fx|port|w1f|x1|x4|fr|fl)/|^worktree-wf_')
in_use=$(git worktree list --porcelain | sed -n 's#^branch refs/heads/##p')
del=""; n_br=0
for b in $all; do
	printf '%s\n' "$in_use" | grep -qxF "$b" && { skip "branch $b (checked out in a remaining worktree)"; continue; }
	if [ "$FORCE" = 1 ] || printf '%s\n' "$merged" | grep -qxF "$b"; then del="$del $b"; n_br=$((n_br + 1))
	else skip "branch $b (not merged into rust-port; --force to delete)"; fi
done
if [ -n "$del" ]; then
	if [ "$YES" = 1 ]; then
		# shellcheck disable=SC2086  # word-splitting the branch list is intended (names have no spaces)
		printf '%s\n' $del | xargs git -C "$here" branch -D >/dev/null && say "  deleted $n_br branches"
	else say "  would delete $n_br branches"; count=$((count + n_br)); fi
fi

say "== 4. Temp directories from tests and gates"
tmp_trash=$tmp/.port-trash-$stamp
n_tmp=0
for d in "$tmp"/deckhand-* "$tmp"/dh-* "$tmp"/dhg-* "$tmp"/dhui* "$tmp"/dhflow-* "$tmp"/dhs-* "$tmp"/int-iso* \
	"$tmp"/parity-dir* "$tmp"/verify-unit-* "$tmp"/wtcheck.* /tmp/dh-* /tmp/dhs-* /tmp/deckhand-*; do
	[ -e "$d" ] || continue
	if [ "$YES" = 1 ]; then trash "$d" "$tmp_trash"; else count=$((count + 1)); fi
	n_tmp=$((n_tmp + 1))
done
# mktemp's default tmp.XXXXXXXX: only those that are empty or look like a Deckhand home/sandbox.
for d in "$tmp"/tmp.*; do
	[ -d "$d" ] || continue
	if [ -z "$(ls -A "$d" 2>/dev/null)" ] || [ -e "$d/state.json" ] || [ -e "$d/daemon.log" ] || [ -e "$d/config.json" ] || [ -d "$d/sandbox" ] || [ -d "$d/h" ]; then
		if [ "$YES" = 1 ]; then trash "$d" "$tmp_trash"; else count=$((count + 1)); fi
		n_tmp=$((n_tmp + 1))
	fi
done
say "  $( [ "$YES" = 1 ] && echo moved || echo 'would remove') $n_tmp temp dirs"

if [ "$DEV_HOMES" = 1 ]; then
	say "== 5. Dev homes"
	for d in "$real_home/.deckhand-rust-dev" "$real_home/.deckhand-rust-copy"; do
		[ -d "$d" ] || continue
		pid=$(cat "$d/daemon.pid" 2>/dev/null || echo "")
		if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then skip "$d (its daemon $pid is running; stop it first)"; continue; fi
		if [ "$YES" = 1 ]; then trash "$d" "$real_home/.port-trash-$stamp"; else plan "$d"; fi
	done
fi

say ""
if [ "$YES" = 0 ]; then say "Dry run complete. Re-run with --yes to remove the items above."; exit 0; fi

# Also pick up trash left by an interrupted earlier run.
for t in "$main_repo"/.claude/.port-trash-* "$tmp"/.port-trash-* "$real_home"/.port-trash-*; do
	[ -d "$t" ] || continue
	case " $trashes " in *" $t "*) ;; *) trashes="$trashes $t" ;; esac
done
[ -z "$trashes" ] && { say "Done. Nothing left to purge."; exit 0; }
purge() { for t in $trashes; do chmod -R u+w "$t" 2>/dev/null; rm -rf "$t"; done; }
if [ "$WAIT" = 1 ]; then
	say "Purging trash (this can take a few minutes for build caches)..."; purge; say "Done."
else
	log=$tmp/port-cleanup-$stamp.log
	( purge > "$log" 2>&1; echo "purge finished $(date)" >> "$log" ) &
	say "Done. The moved files are being deleted in the background (pid $!); progress: $log"
	say "Trash dirs:$trashes"
fi
