import fs from 'node:fs/promises';
import path from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {branchHasOwnCommits, currentBranch, findRepoRoot, git, operationInProgress, optionalGit, resolveDefaultBranch, resolveRepoContext} from './git.js';
import type {MergePreview} from './types.js';
const exec = promisify(execFile);
export interface WorkspaceSummary {
	cwd: string; branch: string; head: string; baseRef?: string;
	changedFiles: number; additions: number; deletions: number; untrackedFiles: number;
	ahead?: number; behind?: number; commitsAheadOfBase?: number;
	/** Where create-pr would push the branch: its upstream remote, else `origin` when it exists. */
	pushRemote?: string;
	pr?: PullRequestInfo;
	prError?: string;
}
export interface PullRequestInfo {
	number: number; url: string; state: string; checks: 'passing' | 'pending' | 'failing' | 'unknown';
	/** The PR's head commit and base branch, as GitHub reports them (merge detection). */
	headSha?: string; baseBranch?: string;
}
/** Looks up the branch's PR in `cwd` (`gh pr view`); injectable so tests never reach GitHub. */
export type PullRequestLookup = (cwd: string) => Promise<PullRequestInfo>;
export interface CleanupInspection {
	safe: boolean; reasons: string[]; dirtyFiles: number; untrackedFiles: number; ignoredFiles: number;
	/** Informational: commits ahead of the cached upstream / comparison base. Only `reasons` decide safety. */
	unpublishedCommits?: number; unmergedCommits?: number;
}
export interface WorkspaceStatus {
	/** HEAD commit; undefined when unborn. */
	oid?: string;
	/** Branch name; undefined when detached. */
	branch?: string;
	upstream?: string; ahead?: number; behind?: number;
	dirtyFiles: number; untracked: string[]; ignored: string[];
	/** Changed (`1`/`2`), unmerged (`u`) and untracked (`?`) entries in Git's order, for the Changes view. */
	entries: StatusEntry[];
}
/**
 * One porcelain-v2 entry. `xy` is Git's two-letter code (index, worktree; `.` unchanged, `??` untracked); `origPath`
 * is a rename's or copy's source. Paths are repository-relative.
 */
export interface StatusEntry {kind: '1' | '2' | 'u' | '?'; xy: string; path: string; origPath?: string}
// Space-separated fields before the path of each porcelain-v2 record kind (the path itself may contain spaces).
const PATH_FIELD: Record<'1' | '2' | 'u', number> = {'1': 8, '2': 9, 'u': 10};
function validateRef(ref: string): string {
	if (!ref || ref.startsWith('-') || /[\0\r\n]/.test(ref)) throw new Error('Invalid Git comparison ref');
	return ref;
}
// `requested` must already have passed validateRef (callers check it before any Git work).
async function base(cwd: string, requested?: string): Promise<string | undefined> {
	const ref = requested || await optionalGit(cwd, ['symbolic-ref', '--quiet', 'refs/remotes/origin/HEAD']);
	if (!ref) return undefined;
	return await optionalGit(cwd, ['rev-parse', '--verify', '--quiet', `${ref}^{commit}`]) ? ref : undefined;
}
/** Parses `git status --porcelain=v2 -z --branch` (paths are repository-relative and may contain spaces/newlines). */
export function parseStatus(raw: string): WorkspaceStatus {
	const status: WorkspaceStatus = {dirtyFiles: 0, untracked: [], ignored: [], entries: []};
	const records = raw.split('\0');
	for (let i = 0; i < records.length; i++) {
		const record = records[i]!;
		const kind = record[0];
		if (kind === '#') {
			const [, key, ...rest] = record.split(' '), value = rest.join(' ');
			if (key === 'branch.oid') status.oid = value === '(initial)' ? undefined : value;
			else if (key === 'branch.head') status.branch = value === '(detached)' ? undefined : value;
			else if (key === 'branch.upstream') status.upstream = value;
			else if (key === 'branch.ab') { const match = value.match(/^\+(\d+) -(\d+)$/); if (match) { status.ahead = Number(match[1]); status.behind = Number(match[2]); } }
		} else if (kind === '1' || kind === '2' || kind === 'u') {
			status.dirtyFiles++;
			const fields = record.split(' ');
			const entry: StatusEntry = {kind, xy: fields[1] ?? '..', path: fields.slice(PATH_FIELD[kind]).join(' ')};
			// Rename/copy: the original path is the next NUL-separated field.
			if (kind === '2') entry.origPath = records[++i] ?? '';
			status.entries.push(entry);
		} else if (kind === '?') { status.untracked.push(record.slice(2)); status.entries.push({kind, xy: '??', path: record.slice(2)}); }
		else if (kind === '!') status.ignored.push(record.slice(2));
	}
	return status;
}
// Cleanup inspection needs ignored files (--ignored=matching collapses ignored directories
// to one entry instead of listing every file); the summary skips that scan.
async function readWorkspaceStatus(cwd: string, includeIgnored: boolean): Promise<WorkspaceStatus> {
	return parseStatus(await git(cwd, ['--no-optional-locks', 'status', '--porcelain=v2', '-z', '--branch', '--untracked-files=all', ...(includeIgnored ? ['--ignored=matching'] : [])], {maxBuffer: 32 * 1024 * 1024}));
}
function numstat(raw: string): {additions: number; deletions: number} {
	let additions = 0, deletions = 0;
	const records = raw.split('\0');
	for (let i = 0; i < records.length; i++) {
		const match = records[i]!.match(/^(\d+|-)\t(\d+|-)\t([\s\S]*)$/);
		if (!match) continue;
		additions += Number(match[1]) || 0; deletions += Number(match[2]) || 0;
		if (!match[3]) i += 2; // Rename: empty path then old and new paths.
	}
	return {additions, deletions};
}
export async function getWorkspaceSummary(cwd: string, baseRef?: string, includePr = false, lookupPr: PullRequestLookup = ghPullRequest): Promise<WorkspaceSummary> {
	if (baseRef) validateRef(baseRef);
	const status = await readWorkspaceStatus(cwd, false);
	const head = status.oid ?? '';
	const [resolvedBase, diff] = await Promise.all([base(cwd, baseRef), git(cwd, ['--no-optional-locks', 'diff', ...(head ? ['HEAD'] : ['--cached']), '--numstat', '-z', '--'])]);
	const result: WorkspaceSummary = {cwd, branch: status.branch ?? '(detached)', head, baseRef: resolvedBase, changedFiles: status.dirtyFiles, untrackedFiles: status.untracked.length, ...numstat(diff)};
	if (status.upstream && status.ahead !== undefined) { result.ahead = status.ahead; result.behind = status.behind; }
	const [aheadOfBase, remote] = await Promise.all([
		resolvedBase && head ? git(cwd, ['rev-list', '--count', `${resolvedBase}..HEAD`]) : undefined,
		status.branch ? pushRemote(cwd, status.branch) : undefined,
		includePr ? lookupPr(cwd).then(pr => { result.pr = pr; }, error => { result.prError = (error as NodeJS.ErrnoException).code === 'ENOENT' ? 'Install gh for PR status' : 'No PR status available; check gh authentication and branch'; }) : undefined,
	]);
	if (aheadOfBase !== undefined) result.commitsAheadOfBase = Number(aheadOfBase) || 0;
	if (remote) result.pushRemote = remote;
	return result;
}
/** The current branch's PR from `gh pr view` (bounded; throws when gh is missing, unauthenticated or finds none). */
export async function ghPullRequest(cwd: string): Promise<PullRequestInfo> {
	const {stdout} = await exec('gh', ['pr', 'view', '--json', 'number,url,state,statusCheckRollup,headRefOid,baseRefName'], {cwd, timeout: 8000, maxBuffer: 1024 * 1024, env: {...process.env, GH_PROMPT_DISABLED: '1'}});
	const pr = JSON.parse(stdout);
	if (!Number.isInteger(pr.number) || typeof pr.url !== 'string' || !/^https:\/\//.test(pr.url)) throw new Error('Invalid PR response');
	const checks = Array.isArray(pr.statusCheckRollup) ? pr.statusCheckRollup : [];
	const failed = checks.some((check: any) => ['FAILURE', 'ERROR', 'CANCELLED', 'TIMED_OUT', 'ACTION_REQUIRED', 'STARTUP_FAILURE'].includes(check.conclusion || check.state));
	const pending = checks.some((check: any) => ['PENDING', 'QUEUED', 'IN_PROGRESS', 'WAITING', 'REQUESTED'].includes(check.state || check.status));
	return {
		number: pr.number, url: pr.url, state: String(pr.state),
		checks: failed ? 'failing' : pending ? 'pending' : checks.length > 0 && checks.every((check: any) => ['SUCCESS', 'NEUTRAL', 'SKIPPED'].includes(check.conclusion || check.state)) ? 'passing' : 'unknown',
		...typeof pr.headRefOid === 'string' && /^[0-9a-f]{40,64}$/.test(pr.headRefOid) ? {headSha: pr.headRefOid} : {},
		...typeof pr.baseRefName === 'string' && pr.baseRefName && !pr.baseRefName.startsWith('-') ? {baseBranch: pr.baseRefName} : {},
	};
}
const disposableIgnored = (file: string) => file.split('/').includes('node_modules');
const MAX_SYMLINK_CHECKS = 2000;
/** lstat-based symlink test relative to the repository root; past the cap entries count as real data (conservative). */
async function symlinkCheck(cwd: string, entries: number): Promise<(file: string) => Promise<boolean>> {
	if (!entries) return async () => false;
	const top = await findRepoRoot(cwd).catch(() => undefined);
	let checks = 0;
	return async file => {
		if (!top || file.endsWith('/') || ++checks > MAX_SYMLINK_CHECKS) return false;
		return fs.lstat(path.join(top, file)).then(stat => stat.isSymbolicLink(), () => false);
	};
}
/**
 * Removing a worktree loses its uncommitted/untracked/valuable ignored files. Commits are kept by the branch,
 * so they only matter when the branch is deleted too or HEAD is detached; then only commits reachable from no
 * other local or remote-tracking branch count. Upstream/base comparisons are informational.
 */
export async function inspectWorkspaceCleanup(cwd: string, baseRef?: string, options: {deleteBranch?: boolean; integrated?: string} = {}): Promise<CleanupInspection> {
	if (baseRef) validateRef(baseRef);
	let status: WorkspaceStatus;
	try { status = await readWorkspaceStatus(cwd, true); }
	catch (error) { return {safe: false, reasons: [`Git status unavailable: ${error instanceof Error ? error.message.split('\n')[0] : String(error)}`], dirtyFiles: 0, untrackedFiles: 0, ignoredFiles: 0}; }
	// Symlinks hold no data of their own (shared dependency dirs, linked env files): removing one never
	// touches its target, so they are disposable. Paths are checked with lstat, never followed.
	const isLink = await symlinkCheck(cwd, status.untracked.length + status.ignored.length);
	const untracked: string[] = [];
	for (const file of status.untracked) if (!await isLink(file)) untracked.push(file);
	const ignored: string[] = [];
	for (const file of status.ignored) if (!disposableIgnored(file) && !await isLink(file)) ignored.push(file);
	const reasons: string[] = [];
	if (status.dirtyFiles) reasons.push(`${status.dirtyFiles} modified/staged file(s)`);
	if (untracked.length) reasons.push(`${untracked.length} untracked file(s)`);
	if (ignored.length) reasons.push(`${ignored.length} valuable ignored file(s), including ${ignored.slice(0, 3).join(', ')}`);
	const checkCommits = Boolean(status.oid) && (options.deleteBranch || !status.branch);
	const exclusive = status.branch ? [`--exclude=${status.branch}`] : []; // --exclude patterns for --branches omit refs/heads/.
	// A squash merge (Deckhand's or a PR's) leaves the branch unmerged as far as Git knows: the commit recorded at merge
	// time and its ancestors are integrated; commits made after it still count.
	const integrated = checkCommits && options.integrated && /^[0-9a-f]{40,64}$/.test(options.integrated) && await optionalGit(cwd, ['rev-parse', '--verify', '--quiet', `${options.integrated}^{commit}`]) ? [options.integrated] : [];
	const [merged, lost] = await Promise.all([
		base(cwd, baseRef).then(resolved => resolved && status.oid ? optionalGit(cwd, ['rev-list', '--count', `${resolved}..HEAD`]) : undefined),
		checkCommits ? optionalGit(cwd, ['rev-list', '--count', 'HEAD', '--not', ...exclusive, '--branches', '--remotes', ...integrated]) : undefined,
	]);
	if (checkCommits) {
		const where = status.branch ? `only on branch ${status.branch}` : 'only on the detached HEAD';
		if (lost === undefined) reasons.push('Commit preservation cannot be verified');
		else if (Number(lost) > 0) reasons.push(`${Number(lost)} commit(s) exist ${where} and would be lost`);
	}
	return {
		safe: reasons.length === 0, reasons, dirtyFiles: status.dirtyFiles, untrackedFiles: untracked.length, ignoredFiles: ignored.length,
		unpublishedCommits: status.upstream && status.ahead !== undefined ? status.ahead : undefined,
		unmergedCommits: merged === undefined ? undefined : Number(merged),
	};
}

/** The branch's configured upstream remote, else `origin` when that remote exists. */
export async function pushRemote(cwd: string, branch: string): Promise<string | undefined> {
	const configured = await optionalGit(cwd, ['config', '--get', `branch.${branch}.remote`]);
	if (configured && configured !== '.' && !configured.startsWith('-')) return configured;
	return (await optionalGit(cwd, ['remote']))?.split('\n').includes('origin') ? 'origin' : undefined;
}
export interface CreatePrResult {branch: string; remote: string; base?: string; existing: boolean}
const firstLine = (error: unknown) => {
	const err = error as Error & {stderr?: string};
	return (err.stderr?.trim() || err.message).split('\n').map(line => line.trim()).filter(Boolean).slice(-3).join(' ').slice(0, 400);
};
const NON_INTERACTIVE = {GIT_TERMINAL_PROMPT: '0', GH_PROMPT_DISABLED: '1', GCM_INTERACTIVE: 'never'};
/** Runs gh in `cwd` with stdin ignored, prompts disabled and a bounded timeout. */
function gh(cwd: string, args: string[], timeout = 60_000): Promise<{stdout: string; stderr: string}> {
	return new Promise((resolve, reject) => {
		const child = execFile('gh', args, {cwd, timeout, maxBuffer: 1024 * 1024, encoding: 'utf8', env: {...process.env, ...NON_INTERACTIVE}}, (error, stdout, stderr) => {
			if (error) reject(Object.assign(error, {stdout, stderr})); else resolve({stdout, stderr});
		});
		child.stdin?.end();
	});
}
/**
 * Pushes the workspace's branch (`git push -u`, never forced) and opens GitHub's new-PR form with `gh pr create --web`,
 * or the existing open PR with `gh pr view --web`. Refuses detached HEAD, main/master and the session's base branch.
 */
export async function createPullRequest(cwd: string, options: {baseRef?: string; expectedBranch?: string} = {}): Promise<CreatePrResult> {
	if (options.baseRef) validateRef(options.baseRef);
	const branch = await git(cwd, ['branch', '--show-current']);
	if (!branch) throw new Error('HEAD is detached; check out a branch before creating a PR');
	if (options.expectedBranch !== undefined && options.expectedBranch !== branch) throw new Error(`Branch changed to ${branch}; reopen i before creating a PR`);
	const remote = await pushRemote(cwd, branch);
	if (!remote) throw new Error(`No remote to push ${branch} to: set an upstream or add an "origin" remote`);
	const baseName = options.baseRef?.startsWith(`${remote}/`) ? options.baseRef.slice(remote.length + 1) : options.baseRef;
	if (branch === 'main' || branch === 'master' || branch === baseName) throw new Error(`Refusing to create a PR from ${branch}: it is ${baseName === branch ? 'the session\'s base branch' : 'a protected branch'}`);
	// --base only when the base exists on the remote as a branch (e.g. origin/main, or a launch branch that was pushed); otherwise gh chooses.
	const base = baseName && /^[^-]/.test(baseName) && await optionalGit(cwd, ['rev-parse', '--verify', '--quiet', `refs/remotes/${remote}/${baseName}`]) ? baseName : undefined;
	try { await git(cwd, ['push', '-u', remote, branch], {timeout: 5 * 60_000, env: {...process.env, ...NON_INTERACTIVE}}); }
	catch (error) { throw new Error(`git push to ${remote} failed: ${firstLine(error)}`); }
	const ghError = (error: unknown) => (error as NodeJS.ErrnoException).code === 'ENOENT'
		? new Error(`Pushed ${branch} to ${remote}, but gh is not installed; install GitHub CLI to open the PR form`)
		: new Error(`Pushed ${branch} to ${remote}, but gh failed: ${firstLine(error)}`);
	let existing = false;
	try {
		const view = JSON.parse((await gh(cwd, ['pr', 'view', branch, '--json', 'url,state'])).stdout);
		existing = view?.state === 'OPEN';
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === 'ENOENT' || !/no (open )?pull requests? found/i.test(firstLine(error))) throw ghError(error);
	}
	try { await gh(cwd, existing ? ['pr', 'view', branch, '--web'] : ['pr', 'create', '--web', '--head', branch, ...(base ? ['--base', base] : [])]); }
	catch (error) { throw ghError(error); }
	return {branch, remote, base, existing};
}

export interface HandoffGitContext {
	baseRef?: string;
	commits: string[]; moreCommits: number;
	changes: Array<{status: string; path: string}>; moreChanges: number;
	diff?: {files: number; insertions: number; deletions: number; names: Array<{path: string; additions?: number; deletions?: number}>};
	error?: string;
}
const MAX_HANDOFF_COMMITS = 30, MAX_HANDOFF_CHANGES = 50;
/** Porcelain v1 -z: `XY path`, renames/copies followed by their original path. */
function changedFiles(raw: string): Array<{status: string; path: string}> {
	const records = raw.split('\0'), changes: Array<{status: string; path: string}> = [];
	for (let i = 0; i < records.length; i++) {
		const record = records[i]!;
		if (record.length < 4) continue;
		const status = record.slice(0, 2);
		changes.push({status: status.trim() || status, path: record.slice(3)});
		if (/[RC]/.test(status[0]!)) i++;
	}
	return changes;
}
function numstatFiles(raw: string): Array<{path: string; additions?: number; deletions?: number}> {
	const records = raw.split('\0'), files: Array<{path: string; additions?: number; deletions?: number}> = [];
	for (let i = 0; i < records.length; i++) {
		const match = records[i]!.match(/^(\d+|-)\t(\d+|-)\t([\s\S]*)$/);
		if (!match) continue;
		let name = match[3]!;
		if (!name) { name = `${records[i + 1] ?? ''} → ${records[i + 2] ?? ''}`; i += 2; }
		files.push({path: name, ...(match[1] === '-' ? {} : {additions: Number(match[1]), deletions: Number(match[2])})});
	}
	return files;
}
/**
 * Git context for a handoff: commits since base, uncommitted file names with status, and a committed diff stat
 * (`base...HEAD`): names and numbers only, never diff content. Undefined outside a Git repository; other
 * failures are returned as `error` so the export still succeeds.
 */
export async function getHandoffGitContext(cwd: string, baseRef?: string): Promise<HandoffGitContext | undefined> {
	if (await optionalGit(cwd, ['rev-parse', '--is-inside-work-tree']) !== 'true') {
		return await fs.stat(cwd).then(() => undefined, error => ({commits: [], moreCommits: 0, changes: [], moreChanges: 0, error: (error as Error).message}));
	}
	try {
		if (baseRef) validateRef(baseRef);
		const [resolvedBase, head, status] = await Promise.all([
			base(cwd, baseRef), optionalGit(cwd, ['rev-parse', '--verify', '--quiet', 'HEAD']),
			git(cwd, ['--no-optional-locks', 'status', '--porcelain=v1', '-z', '--untracked-files=normal'], {maxBuffer: 16 * 1024 * 1024}),
		]);
		const changes = changedFiles(status);
		const context: HandoffGitContext = {baseRef: resolvedBase, commits: [], moreCommits: 0, changes: changes.slice(0, MAX_HANDOFF_CHANGES), moreChanges: Math.max(0, changes.length - MAX_HANDOFF_CHANGES)};
		if (resolvedBase && head) {
			const [log, count, diff] = await Promise.all([
				git(cwd, ['log', '--no-decorate', '--no-color', `--max-count=${MAX_HANDOFF_COMMITS}`, '--format=%h %s', `${resolvedBase}..HEAD`, '--']),
				git(cwd, ['rev-list', '--count', `${resolvedBase}..HEAD`]),
				git(cwd, ['--no-optional-locks', 'diff', '--no-color', '--no-ext-diff', '--numstat', '-z', `${resolvedBase}...HEAD`, '--'], {maxBuffer: 16 * 1024 * 1024}),
			]);
			context.commits = log ? log.split('\n') : [];
			context.moreCommits = Math.max(0, (Number(count) || 0) - context.commits.length);
			const names = numstatFiles(diff);
			context.diff = {files: names.length, insertions: names.reduce((sum, file) => sum + (file.additions ?? 0), 0), deletions: names.reduce((sum, file) => sum + (file.deletions ?? 0), 0), names};
		}
		return context;
	} catch (error) {
		return {baseRef, commits: [], moreCommits: 0, changes: [], moreChanges: 0, error: firstLine(error)};
	}
}

/** Commit subjects shown by a merge preview (the count is exact). */
export const MERGE_PREVIEW_COMMITS = 6;
const MAX_OVERLAP = 200;
/** Every path a numstat -z output names (both sides of a rename). */
function numstatPaths(raw: string): string[] {
	const records = raw.split('\0'), paths: string[] = [];
	for (let i = 0; i < records.length; i++) {
		const match = records[i]!.match(/^(\d+|-)\t(\d+|-)\t([\s\S]*)$/);
		if (!match) continue;
		if (match[3]) paths.push(match[3]);
		else { paths.push(records[i + 1] ?? '', records[i + 2] ?? ''); i += 2; }
	}
	return paths.filter(Boolean);
}
const statusPaths = (status: WorkspaceStatus) => status.entries.flatMap(entry => entry.origPath ? [entry.path, entry.origPath] : [entry.path]);
/**
 * What merging the source worktree into the target would bring (`merge-preview`): commits in `target..source` (count,
 * first subjects), the committed diff stat of `target...source`, the source's uncommitted files, an operation in
 * progress in the target, and the target's uncommitted files the merge would touch. Read-only, bounded.
 */
export async function getMergePreview(sourceCwd: string, targetCwd: string): Promise<MergePreview> {
	const [sourceRoot, targetContext] = await Promise.all([findRepoRoot(sourceCwd), resolveRepoContext(targetCwd)]);
	const targetRoot = targetContext.root;
	if (path.resolve(sourceRoot) === path.resolve(targetRoot)) throw new Error('cannot merge a worktree into itself');
	const [sourceStatus, targetStatus, targetBranch, defaultBranch, inProgress] = await Promise.all([
		readWorkspaceStatus(sourceRoot, false), readWorkspaceStatus(targetRoot, false), currentBranch(targetRoot),
		resolveDefaultBranch(targetRoot), operationInProgress(targetRoot).catch(() => undefined),
	]);
	const sourceSha = sourceStatus.oid;
	const sourceRef = sourceStatus.branch ?? sourceSha ?? 'HEAD';
	const preview: MergePreview = {
		sourceRef, sourceSha, targetRoot, targetBranch: targetBranch || undefined,
		targetIsMain: Boolean(targetContext.mainRoot) && path.resolve(targetContext.mainRoot!) === path.resolve(targetRoot),
		defaultBranch, commitCount: 0, commits: [], diff: {files: 0, insertions: 0, deletions: 0},
		uncommitted: sourceStatus.dirtyFiles + sourceStatus.untracked.length, overlap: {committed: [], uncommitted: []},
		...inProgress ? {inProgress} : {},
	};
	const dirty = new Set(statusPaths(targetStatus));
	const touched = (paths: string[]) => [...new Set(paths.filter(file => dirty.has(file)))].slice(0, MAX_OVERLAP);
	preview.overlap.uncommitted = touched(statusPaths(sourceStatus));
	if (!sourceSha || !targetStatus.oid) return preview;
	const [count, log, diff] = await Promise.all([
		git(targetRoot, ['rev-list', '--count', `HEAD..${sourceSha}`]),
		git(targetRoot, ['log', '--no-decorate', '--no-color', `--max-count=${MERGE_PREVIEW_COMMITS}`, '--format=%s', `HEAD..${sourceSha}`, '--']),
		git(targetRoot, ['--no-optional-locks', 'diff', '--no-color', '--no-ext-diff', '--numstat', '-z', '-M', `HEAD...${sourceSha}`, '--'], {maxBuffer: 16 * 1024 * 1024}),
	]);
	preview.commitCount = Number(count) || 0;
	preview.commits = log ? log.split('\n') : [];
	const files = numstatFiles(diff);
	preview.diff = {files: files.length, insertions: files.reduce((sum, file) => sum + (file.additions ?? 0), 0), deletions: files.reduce((sum, file) => sum + (file.deletions ?? 0), 0)};
	preview.overlap.committed = touched(numstatPaths(diff));
	return preview;
}

export interface AncestryCheck {
	/** Any worktree of the repository. */
	cwd: string;
	/** The branch's tip, its name ('' when detached) and the commit it started from. */
	tip: string; branch: string; start: string;
	/** Fully qualified refs of the default branch, local and `origin/` (whichever exist). */
	defaults: string[];
}
/**
 * Whether the branch was merged outside Deckhand: its tip is reachable from the default branch (local or
 * `origin/<default>`, never fetched) **and** it has a commit of its own beyond its starting point. A fresh branch (no
 * commits: trivially an ancestor of main) never counts, nor one only fast-forwarded or reset to a newer main (its
 * reflog shows no commit made on it). One Git call; the second and third only once the tip looks merged.
 */
export async function mergedIntoDefault({cwd, tip, branch, start, defaults}: AncestryCheck): Promise<boolean> {
	const sha = /^[0-9a-f]{40,64}$/;
	if (!defaults.length || !sha.test(tip) || !sha.test(start) || tip === start) return false;
	const unmerged = await optionalGit(cwd, ['rev-list', '--max-count=1', tip, '--not', start, ...defaults, '--'], {timeout: 5000});
	if (unmerged === undefined || unmerged.trim()) return false;
	// A commit beyond the start: tip descends from it (and differs). A tip reset behind its start does not.
	if (await optionalGit(cwd, ['merge-base', '--is-ancestor', start, tip], {timeout: 5000}) === undefined) return false;
	return branch ? await branchHasOwnCommits(cwd, branch) !== false : true;
}

/**
 * When HEAD was last committed, and how many of its commits none of `defaults` (the default branch, local and `origin/`)
 * has; each absent when Git cannot tell. Bounded: two Git calls.
 */
export async function worktreeActivity(cwd: string, defaults: string[]): Promise<{lastCommitAt?: string; aheadOfDefault?: number}> {
	const [time, ahead] = await Promise.all([
		optionalGit(cwd, ['log', '-1', '--format=%cI', 'HEAD', '--'], {timeout: 5000}),
		defaults.length ? optionalGit(cwd, ['rev-list', '--count', 'HEAD', '--not', ...defaults, '--'], {timeout: 5000}) : undefined,
	]);
	return {...time ? {lastCommitAt: time} : {}, ...ahead !== undefined && /^\d+$/.test(ahead) ? {aheadOfDefault: Number(ahead)} : {}};
}
