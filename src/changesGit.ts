import fs from 'node:fs/promises';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {git} from './git.js';
import {parseStatus} from './workspaceGit.js';
import {changeKey, emptyChanges, firstChangedLine, groupChanges, parseNumstat, untrackedDiff, type ChangeDiff, type ChangeEntry, type ChangeGroup, type ChangesRecord, type LineCounts} from './changesModel.js';

// Git I/O for the Git tab's Changes view, run by the daemon in a workspace root (workspace.ts): one status read
// (porcelain v2 plus numstat), one entry's bounded diff, and staging/unstaging of paths taken from the current
// status only. Never commits, discards or touches the worktree's files.

const MAX_DIFF_BYTES = 256 * 1024;
const MAX_UNTRACKED_COUNTED = 500;
const MAX_UNTRACKED_COUNT_BYTES = 1024 * 1024;
const BINARY_SNIFF_BYTES = 8000;
const READ_TIMEOUT_MS = 8000;
const WRITE_TIMEOUT_MS = 60_000;

/** One status read of a workspace: the record the view shows (capped) plus every entry (for validation). */
export interface ChangesSnapshot {record: ChangesRecord; all: ChangeEntry[]; hasHead: boolean}
/** Untracked line counts by path, reused while size and mtime are unchanged. */
export type UntrackedCounts = Map<string, {size: number; mtimeMs: number; counts: LineCounts}>;

const isBinary = (buffer: Buffer) => buffer.subarray(0, BINARY_SNIFF_BYTES).includes(0);
function countLines(buffer: Buffer): number {
	if (!buffer.length) return 0;
	let lines = 0;
	for (const byte of buffer) if (byte === 10) lines++;
	return buffer[buffer.length - 1] === 10 ? lines : lines + 1;
}

/** Regular files only (never follows symlinks); directories (nested repositories) and big files stay uncounted. */
async function untrackedCounts(cwd: string, files: string[], cache: UntrackedCounts): Promise<Map<string, LineCounts>> {
	const counts = new Map<string, LineCounts>(), seen = new Set<string>();
	for (const file of files.slice(0, MAX_UNTRACKED_COUNTED)) {
		seen.add(file);
		if (file.endsWith('/')) continue;
		const stat = await fs.lstat(path.join(cwd, file)).catch(() => undefined);
		if (!stat?.isFile() || stat.size > MAX_UNTRACKED_COUNT_BYTES) continue;
		const cached = cache.get(file);
		if (cached && cached.size === stat.size && cached.mtimeMs === stat.mtimeMs) { counts.set(file, cached.counts); continue; }
		const buffer = await fs.readFile(path.join(cwd, file)).catch(() => undefined);
		if (!buffer) continue;
		const result: LineCounts = isBinary(buffer) ? {binary: true} : {additions: countLines(buffer), deletions: 0};
		cache.set(file, {size: stat.size, mtimeMs: stat.mtimeMs, counts: result});
		counts.set(file, result);
	}
	for (const file of cache.keys()) if (!seen.has(file)) cache.delete(file);
	return counts;
}

/**
 * Reads the workspace's changes: `git status --porcelain=v2 -z` (no optional locks, so polling never takes the index
 * lock) plus staged/unstaged numstat. Throws when the directory is gone or is not a Git worktree.
 */
export async function readChanges(cwd: string, cache: UntrackedCounts = new Map()): Promise<ChangesSnapshot> {
	if (!await fs.stat(cwd).then(stat => stat.isDirectory(), () => false)) throw new Error('Worktree directory is missing');
	const read = (args: string[]) => git(cwd, ['--no-optional-locks', ...args], {maxBuffer: 32 * 1024 * 1024});
	const [rawStatus, staged, unstaged] = await Promise.all([
		read(['status', '--porcelain=v2', '-z', '--branch', '--untracked-files=all']),
		read(['diff', '--cached', '--numstat', '-z', '-M', '--no-ext-diff', '--']),
		read(['diff', '--numstat', '-z', '--no-ext-diff', '--']),
	]);
	const status = parseStatus(rawStatus);
	const untracked = await untrackedCounts(cwd, status.untracked, cache);
	const numstat = {staged: parseNumstat(staged), unstaged: parseNumstat(unstaged), untracked};
	const all = groupChanges(status.entries, numstat, Number.POSITIVE_INFINITY).entries;
	return {record: {...emptyChanges(), ...groupChanges(status.entries, numstat), loaded: true, branch: status.branch ?? (status.oid ? `(detached ${status.oid.slice(0, 7)})` : undefined)}, all, hasHead: Boolean(status.oid)};
}

/** Runs git with stdout bounded to `maxBytes` (the rest is dropped and the process stopped). */
function boundedGit(cwd: string, args: string[], maxBytes: number): Promise<{text: string; truncated: boolean}> {
	return new Promise((resolve, reject) => {
		const child = spawn('git', ['-C', cwd, ...args], {stdio: ['ignore', 'pipe', 'pipe']});
		const chunks: Buffer[] = [];
		let size = 0, truncated = false, stderr = '';
		const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error('git diff timed out')); }, READ_TIMEOUT_MS);
		child.stdout.on('data', (chunk: Buffer) => {
			if (truncated) return;
			chunks.push(chunk); size += chunk.length;
			if (size > maxBytes) { truncated = true; child.kill(); }
		});
		child.stderr.on('data', (chunk: Buffer) => { stderr = `${stderr}${chunk.toString('utf8')}`.slice(-2000); });
		child.on('error', error => { clearTimeout(timer); reject(error); });
		child.on('close', code => {
			clearTimeout(timer);
			if (code !== 0 && !truncated) { reject(new Error(stderr.trim().split('\n').pop() || `git exited with ${code}`)); return; }
			const text = Buffer.concat(chunks).subarray(0, maxBytes).toString('utf8');
			// A cut output ends at the last complete line.
			resolve({text: truncated ? text.slice(0, Math.max(0, text.lastIndexOf('\n'))) : text, truncated});
		});
	});
}

/**
 * One entry's diff, read-only and bounded: staged → `git diff --cached` (both paths of a rename), unstaged and
 * conflicted → `git diff`, untracked → the file's contents as additions (a symlink shows its target, a directory
 * nothing).
 */
export async function readChangeDiff(cwd: string, entry: ChangeEntry): Promise<ChangeDiff> {
	const base = {group: entry.group, path: entry.path};
	if (entry.group === 'untracked') {
		const file = path.join(cwd, entry.path);
		const stat = await fs.lstat(file);
		if (stat.isSymbolicLink()) { const text = untrackedDiff(`${await fs.readlink(file)}\n`); return {...base, text: `symbolic link\n${text}`, truncated: false, binary: false, firstLine: 1}; }
		if (!stat.isFile()) return {...base, text: '', truncated: false, binary: false, firstLine: 1};
		const handle = await fs.open(file, 'r');
		try {
			const buffer = Buffer.alloc(Math.min(stat.size, MAX_DIFF_BYTES));
			const {bytesRead} = await handle.read(buffer, 0, buffer.length, 0);
			const content = buffer.subarray(0, bytesRead);
			if (isBinary(content)) return {...base, text: '', truncated: false, binary: true, firstLine: 1};
			const truncated = stat.size > bytesRead;
			let text = content.toString('utf8');
			if (truncated) text = text.slice(0, Math.max(0, text.lastIndexOf('\n')));
			return {...base, text: untrackedDiff(text), truncated, binary: false, firstLine: 1};
		} finally { await handle.close(); }
	}
	const paths = entry.group === 'staged' && entry.origPath !== undefined ? [entry.origPath, entry.path] : [entry.path];
	const args = ['--no-optional-locks', '--literal-pathspecs', 'diff', ...(entry.group === 'staged' ? ['--cached', '-M'] : []), '--no-color', '--no-ext-diff', '--', ...paths];
	const {text, truncated} = await boundedGit(cwd, args, MAX_DIFF_BYTES);
	const binary = /^Binary files .* differ$/m.test(text) && !/^@@/m.test(text);
	return {...base, text, truncated, binary, firstLine: firstChangedLine(text)};
}

function gitWithInput(cwd: string, args: string[], input: string): Promise<void> {
	return new Promise((resolve, reject) => {
		const child = spawn('git', ['-C', cwd, ...args], {stdio: ['pipe', 'ignore', 'pipe']});
		let stderr = '';
		const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error(`git ${args.find(arg => !arg.startsWith('-'))} timed out`)); }, WRITE_TIMEOUT_MS);
		child.stderr.on('data', (chunk: Buffer) => { stderr = `${stderr}${chunk.toString('utf8')}`.slice(-2000); });
		child.on('error', error => { clearTimeout(timer); reject(error); });
		child.on('close', code => { clearTimeout(timer); if (code === 0) resolve(); else reject(new Error(stderr.trim().split('\n').filter(Boolean).pop() || `git exited with ${code}`)); });
		child.stdin.end(input);
	});
}
// Paths go through stdin, NUL-separated and literal (no globs or pathspec magic), however many there are.
const pathspecArgs = ['--pathspec-from-file=-', '--pathspec-file-nul'];
const stagePaths = (cwd: string, paths: string[]) => gitWithInput(cwd, ['--literal-pathspecs', 'add', '-A', ...pathspecArgs], paths.join('\0'));
// Without a HEAD commit there is nothing to restore from: remove the paths from the index instead.
const unstagePaths = (cwd: string, paths: string[], hasHead: boolean) => gitWithInput(cwd, hasHead ? ['--literal-pathspecs', 'restore', '--staged', ...pathspecArgs] : ['--literal-pathspecs', 'rm', '--cached', '-r', '-q', '--ignore-unmatch', ...pathspecArgs], paths.join('\0'));

export type StageMode = 'stage' | 'unstage';
/**
 * Stages or unstages one entry (`group` + `path`) or, without a path, everything, validated against `snapshot` (a
 * fresh status read): a path that is not listed there is refused. Stage: `git add -A` of the path (a deletion
 * too; a conflict is marked resolved); stage all: `git add -A`, except that conflicted files are left for one-by-one
 * staging. Unstage: `git restore --staged` (both paths of a rename), `git rm --cached` before the first commit;
 * unstage all: every staged path.
 */
export async function applyStage(cwd: string, snapshot: ChangesSnapshot, mode: StageMode, target?: {group: ChangeGroup; path: string}): Promise<{changed: number; skippedConflicts: number}> {
	const stageable = (entry: ChangeEntry) => entry.group !== 'staged';
	if (target) {
		const entry = snapshot.all.find(candidate => changeKey(candidate) === changeKey(target));
		if (!entry || (mode === 'stage') !== stageable(entry)) throw new Error(`${target.path} is not among the ${mode === 'stage' ? 'unstaged' : 'staged'} changes; refresh and try again`);
		if (mode === 'stage') await stagePaths(cwd, [entry.path]);
		else await unstagePaths(cwd, entry.origPath !== undefined ? [entry.origPath, entry.path] : [entry.path], snapshot.hasHead);
		return {changed: 1, skippedConflicts: 0};
	}
	if (mode === 'unstage') {
		const staged = snapshot.all.filter(entry => entry.group === 'staged');
		if (staged.length) await unstagePaths(cwd, staged.flatMap(entry => entry.origPath !== undefined ? [entry.origPath, entry.path] : [entry.path]), snapshot.hasHead);
		return {changed: staged.length, skippedConflicts: 0};
	}
	const conflicts = snapshot.all.filter(entry => entry.group === 'conflicts').length;
	const pending = snapshot.all.filter(entry => entry.group === 'unstaged' || entry.group === 'untracked');
	if (!conflicts) { if (pending.length) await gitWithInput(cwd, ['add', '-A'], ''); }
	else if (pending.length) await stagePaths(cwd, [...new Set(pending.map(entry => entry.path))]);
	return {changed: new Set(pending.map(entry => entry.path)).size, skippedConflicts: conflicts};
}
