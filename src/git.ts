import fs from 'node:fs/promises';
import os from 'node:os';
import {getConfigDir} from './paths.js';
import path from 'node:path';
import {execFile, spawn} from 'node:child_process';
import type {WorktreeMergeMode, WorktreeMergeResult} from './types.js';
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

async function worktreeEntries(cwd: string): Promise<Array<{path: string; branch: string; head: string; bare: boolean}>> {
	const stdout = await git(cwd, ['worktree', 'list', '--porcelain']);
	const entries: Array<{path: string; branch: string; head: string; bare: boolean}> = [];
	let currentPath = '';
	let currentBranch = '';
	let currentHead = '';
	let isBare = false;

	const flush = () => {
		if (currentPath) entries.push({path: currentPath, branch: currentBranch, head: currentHead, bare: isBare});
		currentPath = '';
		currentBranch = '';
		currentHead = '';
		isBare = false;
	};

	for (const line of stdout.split('\n')) {
		if (line.startsWith('worktree ')) {
			flush();
			currentPath = line.slice('worktree '.length);
		} else if (line.startsWith('HEAD ')) {
			currentHead = line.slice('HEAD '.length);
		} else if (line.startsWith('branch ')) {
			currentBranch = line.slice('branch refs/heads/'.length);
		} else if (line === 'bare') {
			isBare = true;
		} else if (line === '') {
			flush();
		}
	}
	flush();
	return entries;
}

export async function listWorktrees(cwd: string): Promise<WorktreeInfo[]> {
	// The first entry is the main worktree; a bare repository's first entry is not a checkout.
	return (await worktreeEntries(cwd)).flatMap((entry, index) => entry.bare ? [] : [{path: entry.path, branch: entry.branch, head: entry.head, isMain: index === 0}]);
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
/** The new branch's name from `worktree.branchName` (default `{name}`), checked with `git check-ref-format --branch`. */
export async function worktreeBranchName(cwd: string, name: string, template = '{name}'): Promise<string> {
	const branch = expandBranchName(template, {name, user: userSlug()});
	const checked = branchNameProblem(branch) ? undefined : await optionalGit(cwd, ['check-ref-format', '--branch', branch]);
	if (checked !== branch) throw new Error(`Invalid branch name ${JSON.stringify(branch)} from worktree.branchName ${JSON.stringify(template)}`);
	return branch;
}

async function fallbackCreateWorktree(worktreePath: string, name: string, currentWorktreeRoot: string, launchCwd: string, settings?: WorktreeSettings): Promise<{path: string; baseRef?: string}> {
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
	const {start, baseRef} = await branchStart(currentWorktreeRoot, launchCwd, settings?.branchFrom);
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
 * to a newly created worktree.
 */
export async function createWorktreeForSession(title: string, launchCwd: string, creationHook?: CreationHook, settings?: WorktreeSettings): Promise<CreatedWorktreeInfo> {
	const name = sanitizeWorktreeName(title);
	const currentWorktreeRoot = await findRepoRoot(launchCwd);
	const before = pathSet(await listWorktrees(currentWorktreeRoot));
	const creator = creationHook ? 'script' : 'fallback';
	const vars = settings?.location || settings?.symlink?.length || Object.keys(settings?.files ?? {}).length ? await worktreeTemplateVars(name, currentWorktreeRoot) : undefined;
	const {path: worktreePath, baseRef} = creationHook
		? {path: await runCreateScript(creationHook, name, currentWorktreeRoot, launchCwd), baseRef: undefined}
		: await fallbackCreateWorktree(settings?.location && vars ? worktreeLocation(settings.location, vars) : path.join(getConfigDir(), 'worktrees', name), name, currentWorktreeRoot, launchCwd, settings);
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

async function hasUnmergedFiles(cwd: string): Promise<boolean> {
	return (await git(cwd, ['diff', '--name-only', '--diff-filter=U'])).trim().length > 0;
}

export async function mergeWorktreeIntoCurrent(
	worktreePath: string,
	targetCwd: string,
	mode: WorktreeMergeMode,
): Promise<WorktreeMergeResult> {
	const sourceRoot = path.resolve(await findRepoRoot(worktreePath));
	const targetRoot = path.resolve(await findRepoRoot(targetCwd));
	if (sourceRoot === targetRoot) {
		throw new Error('cannot merge a worktree into itself');
	}

	const sourceBranch = await currentBranch(sourceRoot);
	const sourceRef = sourceBranch || await headSha(sourceRoot);
	const targetBranch = await currentBranch(targetRoot);
	if (!targetBranch) {
		throw new Error('target worktree is detached; checkout a branch before merging');
	}

	const commitsToMerge = await countCommitsToMerge(targetRoot, sourceRef);
	if (commitsToMerge === 0) {
		return {
			mode,
			sourceRef,
			targetBranch,
			skipped: true,
			reason: 'No new commits to merge',
			stdout: '',
			stderr: '',
		};
	}

	const args = mode === 'squash'
		? ['merge', '--squash', sourceRef]
		: ['merge', '--no-commit', '--no-ff', sourceRef];
	try {
		const {stdout, stderr} = await gitOutput(targetRoot, args, SLOW);
		return {mode, sourceRef, targetBranch, stdout, stderr};
	} catch (error) {
		const err = error as Error & {stdout?: string; stderr?: string};
		const output = [err.message, err.stdout, err.stderr].filter(Boolean).join('\n').trim();
		if (await hasUnmergedFiles(targetRoot)) {
			return {
				mode,
				sourceRef,
				targetBranch,
				conflicted: true,
				reason: 'Merge has conflicts to resolve',
				stdout: err.stdout ?? '',
				stderr: err.stderr ?? output,
			};
		}
		throw new Error(output || `${mode === 'squash' ? 'squash merge' : 'merge'} failed`);
	}
}
