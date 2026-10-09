import fs from 'node:fs/promises';
import os from 'node:os';
import {getConfigDir} from './paths.js';
import path from 'node:path';
import {execFile, spawn} from 'node:child_process';
import type {TargetOperation, WorktreeMergeMode, WorktreeMergeResult} from './types.js';
import {promisify} from 'node:util';
import {applyWorktreeLinks, branchNameProblem, expandBranchName, userSlug, worktreeLocation, type BranchFrom, type LinkResult, type TemplateVars, type WorktreeSettings} from './worktreeLinks.js';

const execFileAsync = promisify(execFile);

export interface GitOptions {timeout?: number; maxBuffer?: number; env?: NodeJS.ProcessEnv}
/** Bounded reads by default; mutating callers pass a longer timeout. */
export async function gitOutput(cwd: string, args: string[], {timeout = 8000, maxBuffer = 8 * 1024 * 1024, env}: GitOptions = {}): Promise<{stdout: string; stderr: string}> {
	return execFileAsync('git', ['-C', cwd, ...args], {timeout, maxBuffer, encoding: 'utf8', ...(env ? {env} : {})});
}
export async function git(cwd: string, args: string[], options?: GitOptions): Promise<string> {
	return (await gitOutput(cwd, args, options)).stdout.trimEnd();
}
export async function optionalGit(cwd: string, args: string[], options?: GitOptions): Promise<string | undefined> {
	try { return await git(cwd, args, options); } catch { return undefined; }
}
const SLOW: GitOptions = {timeout: 10 * 60_000, maxBuffer: 16 * 1024 * 1024};

export interface WorktreeInfo {
	path: string;
	branch: string;
	head: string;
	isMain: boolean;
	/** `git worktree lock`ed: Git refuses to remove it without a second --force. */
	locked?: boolean;
	/** Listed, but its directory is gone (`git worktree prune` drops it). */
	prunable?: boolean;
}

export interface CreatedWorktreeInfo {
	path: string;
	branch: string;
	head: string;
	isMain: boolean;
	origin: 'created' | 'existing';
	creator: 'script' | 'fallback';
	name: string;
	/** Symlinks applied from worktree settings; only for a newly created worktree. */
	links?: LinkResult;
	/** The base a new branch was started from (branch name, `origin/<default>` or a commit); undefined when reused or hook-created. */
	baseRef?: string;
}

const CREATE_WORKTREE_SCRIPT = path.join('.claude', 'scripts', 'create-worktree.sh');

export async function findRepoRoot(cwd = process.cwd()): Promise<string> {
	return git(cwd, ['rev-parse', '--show-toplevel']);
}

export async function ensureGitRepo(cwd = process.cwd()): Promise<string> {
	try {
		return await findRepoRoot(cwd);
	} catch {
		throw new Error('deckhand must be run from inside a git repository');
	}
}

export async function findGitCommonDir(cwd: string): Promise<string> {
	return git(cwd, ['rev-parse', '--path-format=absolute', '--git-common-dir']);
}

export interface RepoContext {
	/** Checkout containing cwd. */
	root: string;
	commonDir: string;
	/** Main checkout; undefined for bare repositories, which have no shared checkout. */
	mainRoot?: string;
	/** Per-repository key for persisted project trust. */
	trustRoot: string;
}
export async function resolveRepoContext(cwd: string): Promise<RepoContext> {
	const [root = '', gitDir = '', commonDir = ''] = (await git(cwd, ['rev-parse', '--path-format=absolute', '--show-toplevel', '--git-dir', '--git-common-dir'])).split('\n');
	let mainRoot: string | undefined = root;
	if (path.resolve(gitDir) !== path.resolve(commonDir)) {
		const main = (await worktreeEntries(root))[0];
		mainRoot = main && !main.bare ? await fs.realpath(main.path).catch(() => main.path) : undefined;
	}
	// Trust stays keyed by the main checkout root so entries saved by earlier versions stay valid
	// (also correct for submodules, whose common dir lives in the superproject). Bare repositories
	// have no main checkout, so they use their canonical common dir, which is equally unique.
	const trustRoot = mainRoot ?? await fs.realpath(commonDir).catch(() => path.resolve(commonDir));
	return {root, commonDir, mainRoot, trustRoot};
}

interface WorktreeEntry {path: string; branch: string; head: string; bare: boolean; locked: boolean; prunable: boolean}

async function worktreeEntries(cwd: string): Promise<WorktreeEntry[]> {
	const stdout = await git(cwd, ['worktree', 'list', '--porcelain']);
	const entries: WorktreeEntry[] = [];
	let current: WorktreeEntry | undefined;
	const flush = () => {
		if (current?.path) entries.push(current);
		current = undefined;
	};

	for (const line of stdout.split('\n')) {
		if (line.startsWith('worktree ')) {
			flush();
			current = {path: line.slice('worktree '.length), branch: '', head: '', bare: false, locked: false, prunable: false};
		} else if (!current) {
			continue;
		} else if (line.startsWith('HEAD ')) {
			current.head = line.slice('HEAD '.length);
		} else if (line.startsWith('branch ')) {
			current.branch = line.slice('branch refs/heads/'.length);
		} else if (line === 'bare') {
			current.bare = true;
		} else if (line === 'locked' || line.startsWith('locked ')) {
			current.locked = true;
		} else if (line === 'prunable' || line.startsWith('prunable ')) {
			// Git still lists it but its directory is gone.
			current.prunable = true;
		} else if (line === '') {
			flush();
		}
	}
	flush();
	return entries;
}

export async function listWorktrees(cwd: string): Promise<WorktreeInfo[]> {
	// The first entry is the main worktree; a bare repository's first entry is not a checkout.
	return (await worktreeEntries(cwd)).flatMap((entry, index) => entry.bare ? [] : [{
		path: entry.path, branch: entry.branch, head: entry.head, isMain: index === 0,
		...entry.locked ? {locked: true} : {}, ...entry.prunable ? {prunable: true} : {},
	}]);
}

export function sanitizeWorktreeName(title: string): string {
	let sanitized = title
		.toLowerCase()
		.replace(/[^a-z0-9_\-/]+/g, '_')
		.replace(/_+/g, '_')
		.replace(/\/+/g, '/')
		.replace(/^[/_-]+|[/_-]+$/g, '');
	if (!sanitized) {
		sanitized = 'worktree';
	}
	return sanitized.slice(0, 96).replace(/^[/_-]+|[/_-]+$/g, '') || 'worktree';
}

function pathSet(worktrees: WorktreeInfo[]): Set<string> {
	return new Set(worktrees.map(worktree => path.resolve(worktree.path)));
}

async function fileExists(filePath: string): Promise<boolean> {
	try {
		const stat = await fs.stat(filePath);
		return stat.isFile();
	} catch {
		return false;
	}
}

export async function resolveCreateScript(currentWorktreeRoot: string, mainWorktreeRoot?: string): Promise<string | undefined> {
	const candidates = [
		path.join(currentWorktreeRoot, CREATE_WORKTREE_SCRIPT),
		...(mainWorktreeRoot ? [path.join(mainWorktreeRoot, CREATE_WORKTREE_SCRIPT)] : []),
	];
	const seen = new Set<string>();
	for (const candidate of candidates) {
		const resolved = path.resolve(candidate);
		if (seen.has(resolved)) {
			continue;
		}
		seen.add(resolved);
		if (await fileExists(resolved)) {
			return resolved;
		}
	}
	return undefined;
}

export interface CreationHook {file: string; content: string; fingerprint: string}

async function runCreateScript(hook: CreationHook, name: string, cwd: string, launchCwd: string): Promise<string> {
	const stdout = await new Promise<string>((resolve, reject) => {
		// Run the verified in-memory bytes; never let bash re-read a file that may have changed. $0 stays the script path.
		const child = spawn('bash', ['-c', hook.content, hook.file], {
			cwd,
			env: {...process.env, CLAUDE_PROJECT_DIR: launchCwd},
			stdio: ['pipe', 'pipe', 'pipe'],
		});
		let stdoutBuffer = '';
		let stderrBuffer = '';
		const timeout = setTimeout(() => {
			child.kill('SIGTERM');
			reject(new Error('create-worktree.sh timed out after 60s'));
		}, 60_000);
		child.stdout.on('data', chunk => {
			stdoutBuffer += chunk.toString();
		});
		child.stderr.on('data', chunk => {
			stderrBuffer += chunk.toString();
		});
		child.on('error', error => {
			clearTimeout(timeout);
			reject(error);
		});
		child.on('close', code => {
			clearTimeout(timeout);
			if (code === 0) {
				resolve(stdoutBuffer);
				return;
			}
			reject(new Error(`create-worktree.sh failed with exit code ${String(code)}: ${stderrBuffer.trim()}`));
		});
		child.stdin.end(`${JSON.stringify({name, cwd: launchCwd})}\n`);
	});
	const lines = stdout
		.split('\n')
		.map(line => line.trim())
		.filter(Boolean);
	const returnedPath = lines.at(-1);
	if (!returnedPath) {
		throw new Error('create-worktree.sh did not print a worktree path');
	}
	if (!path.isAbsolute(returnedPath)) {
		throw new Error(`create-worktree.sh returned a non-absolute path: ${returnedPath}`);
	}
	return returnedPath;
}

/**
 * The repository's default branch name: origin/HEAD's target, else main, else master. `where` says which refs
 * must exist: local branches (`branchFrom: "default"`) or the remote's branches (before fetching `"origin"`).
 */
export async function resolveDefaultBranch(cwd: string, remote = 'origin', where: 'local' | 'remote' = 'local'): Promise<string | undefined> {
	const head = (await optionalGit(cwd, ['symbolic-ref', '--quiet', `refs/remotes/${remote}/HEAD`]))?.replace(`refs/remotes/${remote}/`, '');
	const exists = async (name: string) => await optionalGit(cwd, ['show-ref', '--verify', '--quiet', where === 'local' ? `refs/heads/${name}` : `refs/remotes/${remote}/${name}`]) !== undefined;
	for (const name of [...head ? [head] : [], 'main', 'master']) if (await exists(name)) return name;
	// Never fetched: the remote may still have it, so let the fetch decide.
	return where === 'remote' ? head ?? 'main' : undefined;
}
const NON_INTERACTIVE_GIT = {GIT_TERMINAL_PROMPT: '0', GCM_INTERACTIVE: 'never'};
async function branchStart(root: string, launchCwd: string, from: BranchFrom = 'current'): Promise<{start: string; baseRef: string}> {
	if (from === 'current') {
		const start = await headSha(launchCwd);
		return {start, baseRef: await currentBranch(launchCwd) || start};
	}
	if (from === 'default') {
		const name = await resolveDefaultBranch(root);
		if (!name) throw new Error('worktree.branchFrom is "default", but the repository has no local default branch (origin/HEAD, main or master)');
		return {start: await git(root, ['rev-parse', '--verify', `refs/heads/${name}^{commit}`]), baseRef: name};
	}
	const remote = 'origin';
	if (!(await optionalGit(root, ['remote']))?.split('\n').includes(remote)) throw new Error('worktree.branchFrom is "origin", but the repository has no origin remote');
	const name = await resolveDefaultBranch(root, remote, 'remote');
	if (!name || name.startsWith('-') || branchNameProblem(name)) throw new Error(`Cannot determine ${remote}'s default branch`);
	// Never fall back to a stale ref: if the fetch fails, creation fails.
	try { await git(root, ['fetch', '--no-tags', remote, `+refs/heads/${name}:refs/remotes/${remote}/${name}`], {timeout: 60_000, env: {...process.env, ...NON_INTERACTIVE_GIT}}); }
	catch (error) {
		const err = error as Error & {stderr?: string};
		throw new Error(`git fetch ${remote} ${name} failed, so the worktree was not created: ${(err.stderr?.trim() || err.message).split('\n').filter(Boolean).slice(-2).join(' ').slice(0, 300)}`);
	}
	return {start: await git(root, ['rev-parse', '--verify', `refs/remotes/${remote}/${name}^{commit}`]), baseRef: `${remote}/${name}`};
}
/** Local branches, most recently committed first (at most `limit`). */
export async function listLocalBranches(cwd: string, limit = 200): Promise<string[]> {
	const out = await optionalGit(cwd, ['for-each-ref', '--sort=-committerdate', `--count=${limit}`, '--format=%(refname:short)', 'refs/heads']);
	return (out ?? '').split('\n').filter(Boolean);
}

/** A chosen base (`baseBranch` of a new session): a local branch, or `origin/<name>` as last fetched (never fetched here). */
async function chosenStart(root: string, base: string): Promise<{start: string; baseRef: string}> {
	if (!base || base.startsWith('-') || branchNameProblem(base)) throw new Error(`Invalid base branch ${JSON.stringify(base)}`);
	const local = await optionalGit(root, ['rev-parse', '--verify', '--quiet', `refs/heads/${base}^{commit}`]);
	if (local) return {start: local, baseRef: base};
	const remote = base.startsWith('origin/') ? await optionalGit(root, ['rev-parse', '--verify', '--quiet', `refs/remotes/${base}^{commit}`]) : undefined;
	if (remote) return {start: remote, baseRef: base};
	throw new Error(`Base branch ${base} does not exist here`);
}

/** The new branch's name from `worktree.branchName` (default `{name}`), checked with `git check-ref-format --branch`. */
export async function worktreeBranchName(cwd: string, name: string, template = '{name}'): Promise<string> {
	const branch = expandBranchName(template, {name, user: userSlug()});
	const checked = branchNameProblem(branch) ? undefined : await optionalGit(cwd, ['check-ref-format', '--branch', branch]);
	if (checked !== branch) throw new Error(`Invalid branch name ${JSON.stringify(branch)} from worktree.branchName ${JSON.stringify(template)}`);
	return branch;
}

async function fallbackCreateWorktree(worktreePath: string, name: string, currentWorktreeRoot: string, launchCwd: string, settings?: WorktreeSettings, baseBranch?: string): Promise<{path: string; baseRef?: string}> {
	const branch = await worktreeBranchName(currentWorktreeRoot, name, settings?.branchName);
	try {
		await fs.access(worktreePath);
		return {path: worktreePath};
	} catch {
		// create it below
	}
	if (await optionalGit(currentWorktreeRoot, ['show-ref', '--verify', '--quiet', `refs/heads/${branch}`]) !== undefined) {
		await fs.mkdir(path.dirname(worktreePath), {recursive: true});
		await git(currentWorktreeRoot, ['worktree', 'add', worktreePath, branch], SLOW);
		return {path: worktreePath};
	}
	const {start, baseRef} = baseBranch ? await chosenStart(currentWorktreeRoot, baseBranch) : await branchStart(currentWorktreeRoot, launchCwd, settings?.branchFrom);
	await fs.mkdir(path.dirname(worktreePath), {recursive: true});
	// --no-track: a remote start point must not become the branch's upstream (create-pr sets that when pushing).
	await git(currentWorktreeRoot, ['worktree', 'add', '--no-track', '-b', branch, worktreePath, start], SLOW);
	return {path: worktreePath, baseRef};
}

/** Placeholder values for worktree templates: the main checkout (or, for bare repositories, the common dir). */
export async function worktreeTemplateVars(name: string, cwd: string): Promise<TemplateVars> {
	const {mainRoot, commonDir} = await resolveRepoContext(cwd);
	const repoRoot = mainRoot ?? await fs.realpath(commonDir).catch(() => path.resolve(commonDir));
	return {name, repo: path.basename(repoRoot).replace(/\.git$/, '') || 'repo', repoParent: path.dirname(repoRoot), repoRoot, home: os.homedir()};
}

/**
 * `creationHook` must be the trusted, already-verified hook (LoadedProject['creationHook']); exactly its bytes are executed.
 * `settings` are the effective worktree settings: the hook (when present) decides the location and branch, otherwise
 * `settings.location` (default ~/.deckhand/worktrees/{name}), `branchFrom` and `branchName` apply; links apply only
 * to a newly created worktree. `baseBranch` (chosen in the new-session form) replaces `branchFrom`; a hook decides
 * the base itself, so it cannot be combined with one.
 */
export async function createWorktreeForSession(title: string, launchCwd: string, creationHook?: CreationHook, settings?: WorktreeSettings, baseBranch?: string): Promise<CreatedWorktreeInfo> {
	if (creationHook && baseBranch) throw new Error("This repository's worktree creation hook decides where new worktrees start, so a base branch cannot be chosen; leave Base on its default");
	const name = sanitizeWorktreeName(title);
	const currentWorktreeRoot = await findRepoRoot(launchCwd);
	const before = pathSet(await listWorktrees(currentWorktreeRoot));
	const creator = creationHook ? 'script' : 'fallback';
	const vars = settings?.location || settings?.symlink?.length || Object.keys(settings?.files ?? {}).length ? await worktreeTemplateVars(name, currentWorktreeRoot) : undefined;
	const {path: worktreePath, baseRef} = creationHook
		? {path: await runCreateScript(creationHook, name, currentWorktreeRoot, launchCwd), baseRef: undefined}
		: await fallbackCreateWorktree(settings?.location && vars ? worktreeLocation(settings.location, vars) : path.join(getConfigDir(), 'worktrees', name), name, currentWorktreeRoot, launchCwd, settings, baseBranch);
	const absolutePath = await fs.realpath(path.resolve(worktreePath));
	const after = await listWorktrees(currentWorktreeRoot);
	const metadata = after.find(worktree => path.resolve(worktree.path) === absolutePath);
	if (!metadata) {
		throw new Error(`created path is not registered as a git worktree: ${absolutePath}`);
	}
	const origin = before.has(absolutePath) ? 'existing' : 'created';
	const links = origin === 'created' && vars && (settings?.symlink?.length || Object.keys(settings?.files ?? {}).length)
		? await applyWorktreeLinks(settings, {launchRoot: currentWorktreeRoot, worktreeRoot: absolutePath, vars})
		: undefined;
	return {...metadata, path: absolutePath, origin, creator, name, ...(links ? {links} : {}), ...(baseRef && origin === 'created' ? {baseRef} : {})};
}

/**
 * Prunes now-empty parents (rmdir only removes empty directories): under Deckhand's own worktree root, and
 * for a custom location the intermediate directories a nested name such as `feat/x` created.
 */
async function cleanupEmptyWorktreeParents(worktreePath: string, name?: string): Promise<void> {
	const worktreesRoot = await fs.realpath(path.resolve(getConfigDir(), 'worktrees')).catch(() => path.resolve(getConfigDir(), 'worktrees'));
	let current = path.dirname(path.resolve(worktreePath));
	let nested = name && worktreePath.endsWith(`${path.sep}${name}`) ? name.split('/').length - 1 : 0;
	while ((current !== worktreesRoot && current.startsWith(`${worktreesRoot}${path.sep}`)) || nested-- > 0) {
		try {
			await fs.rmdir(current);
		} catch {
			break;
		}
		current = path.dirname(current);
	}
}

/**
 * Removes a worktree Git has registered at `worktreePath` (anywhere, including custom locations).
 * `git worktree remove` refuses unregistered paths, so the remnant removal below only ever runs for a
 * directory Git just unregistered. Neither Git nor fs.rm follows symlinks: linked directories keep their contents.
 */
export async function removeWorktree(worktreePath: string, repoCwd: string, name?: string): Promise<void> {
	const absolutePath = path.resolve(worktreePath);
	await git(repoCwd, ['worktree', 'remove', '-f', absolutePath], SLOW);
	// Some hook-created or nested managed worktrees can leave ignored files, empty
	// directories, or parent folders behind after Git unregisters the worktree.
	await fs.rm(absolutePath, {recursive: true, force: true, maxRetries: 3, retryDelay: 100});
	await cleanupEmptyWorktreeParents(absolutePath, name);
	await git(repoCwd, ['worktree', 'prune'], SLOW);
}

/** Drops the administrative entries of worktrees whose directories are gone (every such one in the repository). */
export async function pruneWorktrees(repoCwd: string): Promise<void> {
	await git(repoCwd, ['worktree', 'prune'], SLOW);
}

export async function deleteLocalBranch(repoCwd: string, branch: string): Promise<void> {
	const trimmed = branch.trim();
	if (!trimmed) {
		throw new Error('cannot delete an unnamed branch');
	}
	if (trimmed === 'main' || trimmed === 'master') {
		throw new Error(`refusing to delete protected branch ${trimmed}`);
	}
	await git(repoCwd, ['branch', '-D', trimmed]);
}

/** Empty string when HEAD is detached. */
export async function currentBranch(cwd: string): Promise<string> {
	return git(cwd, ['branch', '--show-current']);
}

export async function headSha(cwd: string): Promise<string> {
	return git(cwd, ['rev-parse', '--verify', 'HEAD']);
}

async function countCommitsToMerge(targetRoot: string, sourceRef: string): Promise<number> {
	return Number.parseInt(await git(targetRoot, ['rev-list', '--count', `HEAD..${sourceRef}`]), 10) || 0;
}

/** At most this many conflicted paths are returned (the count is exact). */
export const MAX_CONFLICT_PATHS = 200;
/** Files with unresolved conflicts (unmerged index entries), repository-relative. */
export async function unmergedFiles(cwd: string): Promise<string[]> {
	return (await git(cwd, ['diff', '--name-only', '-z', '--diff-filter=U'])).split('\0').filter(Boolean);
}

const exists = (file: string) => fs.access(file).then(() => true, () => false);
/**
 * The operation in progress in a worktree, which a merge must not start on top of: a merge (MERGE_HEAD), rebase,
 * cherry-pick or revert, or unresolved conflicts left by one (a kept squash merge has no MERGE_HEAD). Per worktree:
 * `--git-path` resolves inside a linked worktree's own Git directory.
 */
export async function operationInProgress(cwd: string): Promise<TargetOperation | undefined> {
	const [merge = '', rebaseMerge = '', rebaseApply = '', cherryPick = '', revert = ''] = (await git(cwd, ['rev-parse', '--path-format=absolute', '--git-path', 'MERGE_HEAD', '--git-path', 'rebase-merge', '--git-path', 'rebase-apply', '--git-path', 'CHERRY_PICK_HEAD', '--git-path', 'REVERT_HEAD'])).split('\n');
	if (await exists(rebaseMerge) || await exists(rebaseApply)) return 'rebase';
	if (await exists(merge)) return 'merge';
	if (await exists(cherryPick)) return 'cherry-pick';
	if (await exists(revert)) return 'revert';
	return (await unmergedFiles(cwd)).length ? 'conflicts' : undefined;
}
/** Why a merge must not start on top of what the target is doing. */
export function operationProblem(operation: TargetOperation): string {
	return operation === 'conflicts' ? 'The target has unresolved conflicts: resolve and commit them first' : `A ${operation} is in progress in the target: finish or abort it first`;
}

const output = (error: unknown) => {
	const err = error as Error & {stdout?: string; stderr?: string};
	return [err.stdout, err.stderr].filter(Boolean).join('\n').trim() || err.message;
};
/** Uncommitted (changed or untracked, not ignored) files of a worktree. */
export async function uncommittedFiles(cwd: string): Promise<number> {
	const records = (await git(cwd, ['--no-optional-locks', 'status', '--porcelain=v1', '-z', '--untracked-files=all'], {maxBuffer: 32 * 1024 * 1024})).split('\0');
	let files = 0;
	for (let i = 0; i < records.length; i++) {
		if (records[i]!.length < 4) continue;
		files++;
		if (/[RC]/.test(records[i]![0]!)) i++; // A rename's or copy's original path follows.
	}
	return files;
}
/**
 * Commits everything uncommitted in the worktree (`git add -A`, then `git commit -m <message>`; hooks run as usual).
 * Nothing to commit: undefined. A failed commit throws with Git's (and the hooks') output; what `add` staged stays staged.
 */
export async function commitWorktreeChanges(cwd: string, message: string): Promise<{files: number; sha: string} | undefined> {
	const files = await uncommittedFiles(cwd);
	if (!files) return undefined;
	try {
		await git(cwd, ['add', '-A'], SLOW);
		await git(cwd, ['commit', '--no-edit', '-m', message], {...SLOW, env: {...process.env, GIT_EDITOR: 'true'}});
	} catch (error) {
		throw new Error(`Commit failed in the worktree, so nothing was merged:\n${output(error).split('\n').slice(-12).join('\n')}`);
	}
	return {files, sha: await headSha(cwd)};
}

export interface MergeOptions {
	/** Commit the source worktree's uncommitted changes with this message before merging. */
	commitFirst?: string;
}
export async function mergeWorktreeIntoCurrent(
	worktreePath: string,
	targetCwd: string,
	mode: WorktreeMergeMode,
	options: MergeOptions = {},
): Promise<WorktreeMergeResult & {targetRoot: string; targetHead?: string; indexClean?: boolean}> {
	const sourceRoot = path.resolve(await findRepoRoot(worktreePath));
	const targetRoot = path.resolve(await findRepoRoot(targetCwd));
	if (sourceRoot === targetRoot) {
		throw new Error('cannot merge a worktree into itself');
	}

	const sourceBranch = await currentBranch(sourceRoot);
	const targetBranch = await currentBranch(targetRoot);
	if (!targetBranch) {
		throw new Error('target worktree is detached; checkout a branch before merging');
	}
	const operation = await operationInProgress(targetRoot);
	if (operation) throw new Error(operationProblem(operation));
	const committed = options.commitFirst ? await commitWorktreeChanges(sourceRoot, options.commitFirst) : undefined;
	const sourceRef = sourceBranch || await headSha(sourceRoot);
	// Recorded with the marker: the branch moves on, the commit does not (squash merges leave the branch "unmerged").
	const sourceSha = await git(sourceRoot, ['rev-parse', '--verify', `${sourceRef}^{commit}`]);

	const commitsToMerge = await countCommitsToMerge(targetRoot, sourceSha);
	if (commitsToMerge === 0) {
		return {
			mode,
			sourceRef,
			sourceSha,
			targetBranch,
			targetRoot,
			skipped: true,
			reason: 'No new commits to merge',
			...(committed ? {committed} : {}),
			stdout: '',
			stderr: '',
		};
	}

	// A conflicted squash has no MERGE_HEAD; it can be undone (`git reset --merge`) only when the index was clean before,
	// which ort guarantees for a merge it started (it refuses with any staged change). Checked anyway.
	const [targetHead, indexClean] = await Promise.all([headSha(targetRoot), optionalGit(targetRoot, ['diff', '--cached', '--quiet']).then(result => result !== undefined)]);
	const args = mode === 'squash'
		? ['merge', '--squash', sourceRef]
		: ['merge', '--no-commit', '--no-ff', sourceRef];
	try {
		const {stdout, stderr} = await gitOutput(targetRoot, args, SLOW);
		return {mode, sourceRef, sourceSha, targetBranch, targetRoot, ...(committed ? {committed} : {}), stdout, stderr};
	} catch (error) {
		const err = error as Error & {stdout?: string; stderr?: string};
		const message = [err.message, err.stdout, err.stderr].filter(Boolean).join('\n').trim();
		const conflicts = await unmergedFiles(targetRoot).catch(() => []);
		if (conflicts.length) {
			return {
				mode,
				sourceRef,
				sourceSha,
				targetBranch,
				targetRoot,
				targetHead,
				indexClean,
				conflicted: true,
				conflicts: conflicts.slice(0, MAX_CONFLICT_PATHS),
				conflictCount: conflicts.length,
				...(committed ? {committed} : {}),
				reason: 'Merge has conflicts to resolve',
				stdout: err.stdout ?? '',
				stderr: err.stderr ?? message,
			};
		}
		throw new Error(message || `${mode === 'squash' ? 'squash merge' : 'merge'} failed`);
	}
}

/** What Deckhand knows about a conflicted merge it started (to undo a squash, which has no MERGE_HEAD). */
export interface ConflictedMerge {mode: WorktreeMergeMode; targetHead?: string; indexClean?: boolean}
/**
 * Undoes a conflicted merge in `targetRoot`. With MERGE_HEAD: `git merge --abort`. A squash merge has none, so
 * `git reset --merge` (what `merge --abort` runs) restores the index and the files the merge changed while keeping
 * unrelated unstaged edits and untracked files; that is only safe for a squash Deckhand started on a clean index
 * whose HEAD has not moved since, otherwise it refuses.
 */
export async function abortMerge(targetRoot: string, started?: ConflictedMerge): Promise<void> {
	const mergeHead = await git(targetRoot, ['rev-parse', '--path-format=absolute', '--git-path', 'MERGE_HEAD']);
	if (await exists(mergeHead)) {
		await git(targetRoot, ['merge', '--abort'], SLOW);
		return;
	}
	if (started?.mode !== 'squash' || !started.indexClean || !started.targetHead || started.targetHead !== await headSha(targetRoot).catch(() => undefined)) {
		throw new Error('Cannot abort this squash merge safely (Deckhand did not start it on a clean index, or the target changed since); undo it in Git');
	}
	await git(targetRoot, ['reset', '--merge'], SLOW);
	await fs.rm(await git(targetRoot, ['rev-parse', '--path-format=absolute', '--git-path', 'SQUASH_MSG']), {force: true});
}

/**
 * The commit a branch was created at, from its reflog's oldest entry when that is the creation (`branch: Created
 * from …`); undefined when the reflog is gone or starts later.
 */
export async function branchCreationCommit(cwd: string, branch: string): Promise<string | undefined> {
	if (!branch || branch.startsWith('-')) return undefined;
	const lines = (await optionalGit(cwd, ['log', '-g', '--format=%H %gs', `refs/heads/${branch}`, '--']))?.split('\n').filter(Boolean) ?? [];
	const oldest = lines.at(-1)?.match(/^([0-9a-f]{40,64}) branch: Created from /);
	return oldest?.[1];
}

/** Whether the branch's reflog shows at least one commit made on it (`commit`, `commit (amend)`, `commit (merge)`, …). */
export async function branchHasOwnCommits(cwd: string, branch: string): Promise<boolean | undefined> {
	const subjects = await optionalGit(cwd, ['log', '-g', '--format=%gs', `refs/heads/${branch}`, '--']);
	if (subjects === undefined || !subjects.trim()) return undefined;
	// A fast-forward, reset or rebase moves the branch without a commit of its own (e.g. a fresh branch brought up to date).
	return subjects.split('\n').some(subject => /^(commit|cherry-pick)\b/.test(subject) || /^(merge|pull)\b.*: Merge made by/.test(subject));
}
