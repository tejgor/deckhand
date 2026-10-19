#!/bin/sh
# Port of scripts/deckhand-dev.mjs: the isolated dev launcher (and its disposable sandbox) in POSIX sh.
# Usage: scripts/deckhand-dev.sh [--source] [--sandbox] [status|stop]   (see --help)
#
# Sourcing this file with DECKHAND_DEV_LIB=1 defines isolated_home / create_sandbox (the TS exports isolatedHome /
# createSandbox) without running main, which is how the tests exercise them.
#
# Deviation from the TS: the TS launcher spawns the CLI and forwards SIGINT/SIGTERM; a POSIX shell cannot do that
# without breaking the child's terminal (background jobs lose stdin and ignore SIGINT), so main `exec`s the CLI
# instead. Signals, the controlling terminal and the exit status then belong to the CLI itself.

project=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd -P)

dev_home_dir() {
	if [ -n "${HOME:-}" ]; then printf '%s' "$HOME"; else (cd ~ 2>/dev/null && pwd); fi
}

# path.resolve for one absolute-or-relative path: lexical only (`.` and `..` collapse without looking at symlinks).
lexical_resolve() {
	case $1 in /*) set -- "$1" ;; *) set -- "$(pwd -P)/$1" ;; esac
	_out=''
	_old_ifs=$IFS
	IFS=/
	set -f
	# shellcheck disable=SC2086
	set -- $1
	set +f
	IFS=$_old_ifs
	for _part do
		case $_part in
			'' | .) ;;
			..) _out=${_out%/*} ;;
			*) _out=$_out/$_part ;;
		esac
	done
	printf '%s' "${_out:-/}"
}

# fs.realpath that also resolves paths which do not exist yet (the existing prefix is resolved, dangling symlinks
# followed, the missing tail appended). Prints the result; returns 1 with a message on a symlink loop.
canonical() {
	_rest=$(lexical_resolve "$1")
	_resolved=''
	_links=0
	while [ -n "$_rest" ]; do
		_rest=${_rest#/}
		case $_rest in
			*/*) _part=${_rest%%/*}; _rest=${_rest#*/} ;;
			*) _part=$_rest; _rest='' ;;
		esac
		case $_part in
			'' | .) continue ;;
			..) _resolved=${_resolved%/*}; continue ;;
		esac
		_candidate=$_resolved/$_part
		if [ -L "$_candidate" ]; then
			_links=$((_links + 1))
			if [ "$_links" -gt 40 ]; then
				printf 'ELOOP: too many symbolic links encountered, stat %s\n' "$1" >&2
				return 1
			fi
			_target=$(readlink -- "$_candidate")
			case $_target in
				/*) _resolved='' ;;
			esac
			_rest=$_target${_rest:+/$_rest}
			continue
		fi
		_resolved=$_candidate
	done
	printf '%s' "${_resolved:-/}"
}

# TS isolatedHome. Prints the canonical dev state directory; refuses (message on stderr, status 1) one that is,
# contains or sits inside ~/.deckhand or DECKHAND_HOME, symlinks included.
isolated_home() {
	_user_home=$(dev_home_dir)
	if [ $# -gt 0 ]; then _candidate=$1; else _candidate=${DECKHAND_DEV_HOME:-$_user_home/.deckhand-dev}; fi
	# Production state is ~/.deckhand, plus DECKHAND_HOME when this shell points stable Deckhand elsewhere
	# (inside an isolated dev session DECKHAND_HOME is the dev home itself).
	_configured=''
	if [ "${DECKHAND_CHANNEL:-}" != dev ]; then _configured=${DECKHAND_HOME:-}; fi
	_production=$(canonical "$_user_home/.deckhand") || return 1
	_configured_canonical=''
	if [ -n "$_configured" ]; then _configured_canonical=$(canonical "$_configured") || return 1; fi
	_isolated=$(canonical "$_candidate") || return 1
	for _home in "$_production" ${_configured_canonical:+"$_configured_canonical"}; do
		case $_isolated in
			"$_home" | "$_home"/*) _clash=1 ;;
			*) _clash='' ;;
		esac
		case $_home in
			"$_isolated"/*) _clash=1 ;;
		esac
		if [ -n "$_clash" ]; then
			printf '%s\n' 'The dev state directory must be separate from ~/.deckhand and DECKHAND_HOME (including symlinks)' >&2
			return 1
		fi
	done
	printf '%s' "$_isolated"
}

new_uuid() {
	if command -v uuidgen >/dev/null 2>&1; then uuidgen | tr 'A-Z' 'a-z'
	elif [ -r /proc/sys/kernel/random/uuid ]; then cat /proc/sys/kernel/random/uuid
	else od -An -N16 -tx1 /dev/urandom | tr -d ' \n'
	fi
}

# Runs git in the sandbox under construction; a failure reads like node's execFile rejection.
sandbox_git() {
	_dir=$1; shift
	if ! _git_out=$(git -C "$_dir" "$@" 2>&1); then
		printf 'Command failed: git -C %s %s\n%s\n' "$_dir" "$*" "$_git_out" >&2
		return 1
	fi
}

populate_sandbox() {
	_cwd=$1; _final=$2
	mkdir -p -- "$_cwd" || return 1
	cat >"$_cwd/package.json" <<'EOF' || return 1
{
  "name": "deckhand-sandbox",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "node --test",
    "dev": "node app.js"
  }
}
EOF
	cat >"$_cwd/app.js" <<'EOF' || return 1
import http from 'node:http';
http.createServer((request, response) => response.end('Hello from the Deckhand dev sandbox!')).listen(4319, '127.0.0.1');
console.log('Sandbox server: http://127.0.0.1:4319');
EOF
	cat >"$_cwd/app.test.js" <<'EOF' || return 1
import {test} from 'node:test';
import assert from 'node:assert/strict';
test('example', () => assert.equal(2 + 2, 4));
EOF
	cat >"$_cwd/deckhand.json" <<'EOF' || return 1
{
  "defaultWorkspace": "new",
  "devCommand": "npm run dev",
  "actions": {
    "test": "npm test"
  }
}
EOF
	cat >"$_cwd/README.md" <<'EOF' || return 1
# Deckhand dev sandbox

A disposable repository for testing the isolated workbench. No dependencies to install.
EOF
	sandbox_git "$_cwd" init -b main &&
		sandbox_git "$_cwd" config user.name 'Deckhand Sandbox' &&
		sandbox_git "$_cwd" config user.email 'sandbox@deckhand.invalid' &&
		sandbox_git "$_cwd" config commit.gpgsign false &&
		sandbox_git "$_cwd" config core.hooksPath "$_final/.git/no-hooks" &&
		sandbox_git "$_cwd" add . &&
		sandbox_git "$_cwd" commit -m 'Initialize disposable sandbox'
}

# TS createSandbox. Prints the sandbox directory (<home>/sandbox), creating it first if needed.
create_sandbox() {
	_home=$1
	_sandbox=$_home/sandbox
	if [ -e "$_sandbox/.git" ]; then printf '%s' "$_sandbox"; return 0; fi
	if [ -e "$_sandbox" ]; then
		printf 'Refusing to overwrite existing sandbox directory: %s\n' "$_sandbox" >&2
		return 1
	fi
	# Build aside and rename into place so an interrupted first run never leaves a half-made sandbox behind.
	_temporary=$_home/sandbox.tmp-$(new_uuid)
	if populate_sandbox "$_temporary" "$_sandbox" && ! [ -e "$_sandbox" ] && ! [ -L "$_sandbox" ] && mv -- "$_temporary" "$_sandbox"; then
		printf '%s' "$_sandbox"
		return 0
	fi
	_status=$?
	rm -rf -- "$_temporary"
	if [ -e "$_sandbox/.git" ]; then printf '%s' "$_sandbox"; return 0; fi # A concurrent launch won the race.
	return "$_status"
}

usage() {
	printf '%s\n' 'Usage: scripts/deckhand-dev.sh [--source] [--sandbox] [status|stop]' '' \
		'Built CLI by default; --source marks the run as a development build (DECKHAND_DEV=1). State: ~/.deckhand-dev, override with DECKHAND_DEV_HOME.' \
		'--sandbox opens a disposable Git repo. No cargo install/global install required.' \
		'DECKHAND_TEST_BIN=<path> runs that deckhand executable instead (e.g. target/debug/deckhand).'
}

die() { printf '%s\n' "$1" >&2; exit 1; }

# TS main.
main() {
	for _arg do
		if [ "$_arg" = --help ]; then usage; return 0; fi
	done
	_home=$(isolated_home) || exit 1
	(umask 077 && mkdir -p -- "$_home") || die "EACCES: permission denied, mkdir '$_home'"
	_source=''
	_sandbox_flag=''
	for _arg do
		case $_arg in
			--source) _source=1 ;;
			--sandbox) _sandbox_flag=1 ;;
		esac
	done
	if [ -n "$_sandbox_flag" ]; then _cwd=$(create_sandbox "$_home") || exit 1; else _cwd=$(pwd -P); fi
	# Drop the launcher's own flags, keeping the rest (and their spacing) intact.
	_count=$#
	while [ "$_count" -gt 0 ]; do
		_arg=$1; shift
		case $_arg in
			--source | --sandbox) ;;
			*) set -- "$@" "$_arg" ;;
		esac
		_count=$((_count - 1))
	done
	case ${1:-} in
		setup | doctor)
			_check=''
			for _arg do [ "$_arg" = --check ] && _check=1; done
			[ -n "$_check" ] || die 'The isolated launcher does not install global agent tools. Use setup --check for read-only inspection.'
			;;
	esac
	# DECKHAND_TEST_BIN (absolute, or relative to this checkout) runs that executable, e.g. a native build under test.
	# Without it the release build is preferred over the debug one.
	_test_bin=''
	if [ -n "${DECKHAND_TEST_BIN:-}" ]; then
		case $DECKHAND_TEST_BIN in
			/*) _test_bin=$(lexical_resolve "$DECKHAND_TEST_BIN") ;;
			*) _test_bin=$(lexical_resolve "$project/$DECKHAND_TEST_BIN") ;;
		esac
		_entry=$_test_bin
	elif [ -e "$project/target/release/deckhand" ]; then
		_entry=$project/target/release/deckhand
	else
		_entry=$project/target/debug/deckhand
	fi
	if ! [ -e "$_entry" ]; then
		if [ -n "$_test_bin" ]; then die "DECKHAND_TEST_BIN not found: $_test_bin"; else die 'Build this checkout first: cargo build'; fi
	fi
	DECKHAND_HOME=$_home
	DECKHAND_CHANNEL=dev
	if [ -n "$_source" ]; then DECKHAND_DEV=1; else DECKHAND_DEV=0; fi
	export DECKHAND_HOME DECKHAND_CHANNEL DECKHAND_DEV
	unset DECKHAND_SESSION_ID DECKHAND_LAUNCH_ID DECKHAND_HOOK_TOKEN
	printf 'Deckhand dev · isolated state: %s\nWorkspace: %s\n' "$_home" "$_cwd" >&2
	cd -- "$_cwd" 2>/dev/null || die "spawn $_entry ENOENT"
	exec "$_entry" "$@"
}

if [ -z "${DECKHAND_DEV_LIB:-}" ]; then main "$@"; fi
