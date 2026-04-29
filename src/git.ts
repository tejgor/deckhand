import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {execFile, spawn} from 'node:child_process';
import type {WorktreeMergeMode, WorktreeMergeResult} from './types.js';
import {promisify} from 'node:util';

const execFileAsync = promisify(execFile);

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
}

const CREATE_WORKTREE_SCRIPT = path.join('.claude', 'scripts', 'create-worktree.sh');

export async function findRepoRoot(cwd = process.cwd()): Promise<string> {
	const {stdout} = await execFileAsync('git', ['-C', cwd, 'rev-parse', '--show-toplevel']);
	return stdout.trim();
}

export async function ensureGitRepo(cwd = process.cwd()): Promise<string> {
	try {
		return await findRepoRoot(cwd);
	} catch {
		throw new Error('deckhand must be run from inside a git repository');
	}
}

export async function findGitCommonDir(cwd: string): Promise<string> {
	const {stdout} = await execFileAsync('git', ['-C', cwd, 'rev-parse', '--path-format=absolute', '--git-common-dir']);
	return stdout.trim();
}

export async function findMainWorktreeRoot(cwd: string): Promise<string> {
	const commonDir = await findGitCommonDir(cwd);
	const candidate = path.resolve(commonDir, '..');
	try {
		return await findRepoRoot(candidate);
	} catch {
		return candidate;
	}
}

export async function listWorktrees(cwd: string): Promise<WorktreeInfo[]> {
	const {stdout} = await execFileAsync('git', ['-C', cwd, 'worktree', 'list', '--porcelain']);
	const worktrees: WorktreeInfo[] = [];
	let currentPath = '';
	let currentBranch = '';
	let currentHead = '';
	let isBare = false;
	let isFirst = true;

	const flush = () => {
		if (currentPath && !isBare) {
			worktrees.push({
				path: currentPath,
				branch: currentBranch,
				head: currentHead,
				isMain: isFirst,
			});
			isFirst = false;
		}
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
	return worktrees;
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

async function resolveCreateScript(currentWorktreeRoot: string, mainWorktreeRoot: string): Promise<string | undefined> {
	const candidates = [
		path.join(currentWorktreeRoot, CREATE_WORKTREE_SCRIPT),
		path.join(mainWorktreeRoot, CREATE_WORKTREE_SCRIPT),
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

async function runCreateScript(scriptPath: string, name: string, cwd: string, launchCwd: string): Promise<string> {
	const stdout = await new Promise<string>((resolve, reject) => {
		const child = spawn('bash', [scriptPath], {
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

async function fallbackCreateWorktree(name: string, currentWorktreeRoot: string, launchCwd: string): Promise<string> {
	const worktreesDir = path.join(os.homedir(), '.deckhand', 'worktrees');
	const worktreePath = path.join(worktreesDir, name);
	await fs.mkdir(path.dirname(worktreePath), {recursive: true});
	try {
		await fs.access(worktreePath);
		return worktreePath;
	} catch {
		// create it below
	}
	const {stdout: startStdout} = await execFileAsync('git', ['-C', launchCwd, 'rev-parse', 'HEAD']);
	const start = startStdout.trim();
	let branchExists = false;
	try {
		await execFileAsync('git', ['-C', currentWorktreeRoot, 'show-ref', '--verify', '--quiet', `refs/heads/${name}`]);
		branchExists = true;
	} catch {
		branchExists = false;
	}
	if (branchExists) {
		await execFileAsync('git', ['-C', currentWorktreeRoot, 'worktree', 'add', worktreePath, name]);
	} else {
		await execFileAsync('git', ['-C', currentWorktreeRoot, 'worktree', 'add', '-b', name, worktreePath, start]);
	}
	return worktreePath;
}

export async function createWorktreeForSession(title: string, launchCwd: string): Promise<CreatedWorktreeInfo> {
	const name = sanitizeWorktreeName(title);
	const currentWorktreeRoot = await findRepoRoot(launchCwd);
	const mainWorktreeRoot = await findMainWorktreeRoot(launchCwd);
	const before = pathSet(await listWorktrees(currentWorktreeRoot));
	const scriptPath = await resolveCreateScript(currentWorktreeRoot, mainWorktreeRoot);
	const creator = scriptPath ? 'script' : 'fallback';
	const worktreePath = scriptPath
		? await runCreateScript(scriptPath, name, currentWorktreeRoot, launchCwd)
		: await fallbackCreateWorktree(name, currentWorktreeRoot, launchCwd);
	const absolutePath = path.resolve(worktreePath);
	const after = await listWorktrees(currentWorktreeRoot);
	const metadata = after.find(worktree => path.resolve(worktree.path) === absolutePath);
	if (!metadata) {
		throw new Error(`created path is not registered as a git worktree: ${absolutePath}`);
	}
	return {
		...metadata,
		path: absolutePath,
		origin: before.has(absolutePath) ? 'existing' : 'created',
		creator,
		name,
	};
}

export async function removeWorktree(worktreePath: string, repoCwd: string): Promise<void> {
	await execFileAsync('git', ['-C', repoCwd, 'worktree', 'remove', '-f', worktreePath]);
	await execFileAsync('git', ['-C', repoCwd, 'worktree', 'prune']);
}

async function currentBranch(cwd: string): Promise<string> {
	const {stdout} = await execFileAsync('git', ['-C', cwd, 'branch', '--show-current']);
	return stdout.trim();
}

async function headSha(cwd: string): Promise<string> {
	const {stdout} = await execFileAsync('git', ['-C', cwd, 'rev-parse', 'HEAD']);
	return stdout.trim();
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

	const args = mode === 'squash'
		? ['merge', '--squash', sourceRef]
		: ['merge', '--no-commit', '--no-ff', sourceRef];
	try {
		const {stdout, stderr} = await execFileAsync('git', ['-C', targetRoot, ...args], {maxBuffer: 10 * 1024 * 1024});
		return {mode, sourceRef, targetBranch, stdout, stderr};
	} catch (error) {
		const err = error as Error & {stdout?: string; stderr?: string};
		const output = [err.message, err.stdout, err.stderr].filter(Boolean).join('\n').trim();
		throw new Error(output || `${mode === 'squash' ? 'squash merge' : 'merge'} failed`);
	}
}
