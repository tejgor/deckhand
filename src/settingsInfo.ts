import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {execFile} from 'node:child_process';
import {hookCommand} from './agentSignals.js';
import {git, optionalGit, resolveCreateScript, resolveDefaultBranch, resolveRepoContext, worktreeTemplateVars, type RepoContext} from './git.js';
import {explainSettings, isProjectTrusted, loadProjectConfig, projectNeedsReview, validateProjectConfig, PROJECT_CONFIG_FILE, type LoadedProject, type SettingRow} from './projectConfig.js';
import {readConfigTargets, type ConfigTargetKind, type ConfigTargets} from './projectConfigDocument.js';
import {parseStatus} from './workspaceGit.js';
import {getConfigDir} from './paths.js';
import {loadAppConfig} from './storage.js';
import {mergeWorktreeSettings, userSlug, validRelativePath, type TemplateVars, type WorktreeSettings} from './worktreeLinks.js';

// Daemon-side readers for the Settings screen (C): the effective settings with their sources plus both editable
// documents, the link candidates of the main checkout and their bounded sizes. Saving goes through save-config.

/** C → Settings for `cwd`'s repository. Read-only; `targets` carries the documents (and revisions) edits are saved to. */
export interface SettingsInfo {
	cwd: string;
	repo: string;
	repository: {state: 'trusted' | 'untrusted' | 'absent' | 'invalid' | 'bare' | 'none'; path?: string; error?: string};
	/** T has something to review (an untrusted deckhand.json or enabled creation hook). */
	needsReview: boolean;
	globalError?: string;
	/** explainSettings: each effective value with its source and any pending (untrusted) repository value. */
	rows: SettingRow[];
	targets: ConfigTargets;
	/** A detected .claude/scripts/create-worktree.sh (enabled or not). */
	hookFile?: string;
	/** Template placeholders with `<name>` for previews; absent outside a Git repository. */
	vars?: TemplateVars;
	/** The built-in worktree location for `<name>`. */
	defaultLocation: string;
	/** Whether `.worktrees/` in the main checkout is gitignored (the "inside repo" location preset). */
	insideIgnored: boolean;
	defaultBranch?: string;
	/** origin's default branch name when an origin remote exists. */
	originBranch?: string;
	user: string;
	/** User config.json flags (global only): agent_hooks and notifications. */
	agentHooks: boolean;
	notifications: boolean;
	/** Codex is used here but its hooks don't call this Deckhand (see codexHookState); absent when fine or irrelevant. */
	codexHooks?: CodexHookStatus;
}

/** Codex's own hook config lacks this Deckhand's bridge ('missing') or calls another install of it ('other'). */
export interface CodexHookStatus {state: 'missing' | 'other'; /** ~/.codex/hooks.json (or $CODEX_HOME's). */ file: string; fileExists: boolean; /** What prints the hook config. */ command: string}
/** Every hook `command` in Codex's hooks.json and config.toml texts (TOML strings read loosely; this is only a hint). */
export function codexHookCommands(hooksJson: string | undefined, configToml: string | undefined): string[] {
	const commands: string[] = [];
	const walk = (value: unknown): void => {
		if (Array.isArray(value)) value.forEach(walk);
		else if (value && typeof value === 'object') for (const [key, child] of Object.entries(value)) { if (key === 'command' && typeof child === 'string') commands.push(child); else walk(child); }
	};
	try { if (hooksJson) walk(JSON.parse(hooksJson)); } catch {}
	for (const match of (configToml ?? '').matchAll(/^\s*command\s*=\s*("(?:[^"\\]|\\.)*"|'[^']*')/gm)) {
		const quoted = match[1]!;
		try { commands.push(quoted.startsWith("'") ? quoted.slice(1, -1) : JSON.parse(quoted) as string); } catch {}
	}
	return commands;
}
/** Pure: whether `commands` include `expected` (this Deckhand's bridge), another Deckhand bridge, or none. */
export function codexHookState(commands: string[], expected: string): CodexHookStatus['state'] | undefined {
	if (commands.includes(expected)) return undefined;
	return commands.some(command => /deckhand/i.test(command) && /\bhook'?\s*$/.test(command)) ? 'other' : 'missing';
}
async function readCodexHooks(): Promise<CodexHookStatus | undefined> {
	const dir = process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
	// No Codex home: Codex isn't used here, so there is nothing to hint.
	if (!await fs.stat(dir).then(stat => stat.isDirectory(), () => false)) return undefined;
	const file = path.join(dir, 'hooks.json');
	const [hooksJson, configToml] = await Promise.all([file, path.join(dir, 'config.toml')].map(name => fs.readFile(name, 'utf8').catch(() => undefined)));
	const state = codexHookState(codexHookCommands(hooksJson, configToml), hookCommand());
	const command = process.env.DECKHAND_CHANNEL === 'dev' ? 'node scripts/deckhand-dev.mjs hooks codex' : 'deckhand hooks codex';
	return state && {state, file, fileExists: hooksJson !== undefined, command};
}

const message = (error: unknown) => error instanceof Error ? error.message : String(error);
export async function readSettingsInfo(cwd: string): Promise<SettingsInfo> {
	const user = await loadAppConfig();
	let context: RepoContext | undefined, project: LoadedProject | undefined, error: string | undefined;
	try { context = await resolveRepoContext(cwd); } catch (caught) { error = `Not a Git repository: ${message(caught)}`; }
	if (context) try { project = await loadProjectConfig(cwd, user, context); } catch (caught) { error = message(caught); }
	const checkout = context ? context.mainRoot ?? context.root : undefined;
	const [hookFile, vars, targets, insideIgnored, defaultBranch, remotes] = await Promise.all([
		project ? Promise.resolve(project.creationHook?.file ?? project.disabledHook?.file) : context ? resolveCreateScript(context.root, context.mainRoot).catch(() => undefined) : Promise.resolve(undefined),
		context ? worktreeTemplateVars('<name>', cwd).catch(() => undefined) : Promise.resolve(undefined),
		readConfigTargets(cwd),
		context?.mainRoot ? optionalGit(context.mainRoot, ['check-ignore', '-q', '--', '.worktrees/probe']).then(result => result !== undefined) : Promise.resolve(false),
		checkout ? resolveDefaultBranch(checkout).catch(() => undefined) : Promise.resolve(undefined),
		checkout ? optionalGit(checkout, ['remote']) : Promise.resolve(undefined),
	]);
	const codexHooks = user.agent_hooks ? await readCodexHooks().catch(() => undefined) : undefined;
	const originBranch = checkout && remotes?.split('\n').includes('origin') ? await resolveDefaultBranch(checkout, 'origin', 'remote').catch(() => undefined) : undefined;
	const {rows, globalError} = explainSettings(project, user, {...vars ? {vars} : {}, user: userSlug(), ...hookFile ? {hookFile} : {}});
	const state: SettingsInfo['repository']['state'] = !context ? 'none' : error ? 'invalid' : !project?.path ? 'bare' : !project.exists ? 'absent' : isProjectTrusted(project, user) ? 'trusted' : 'untrusted';
	const file = project?.path ?? (context?.mainRoot ? path.join(context.mainRoot, PROJECT_CONFIG_FILE) : undefined);
	return {
		cwd, repo: vars?.repo ?? path.basename(cwd),
		repository: {state, ...file ? {path: file} : {}, ...error ? {error} : {}},
		needsReview: Boolean(project && projectNeedsReview(project, user)), ...globalError ? {globalError} : {}, rows, targets,
		...hookFile ? {hookFile} : {}, ...vars ? {vars} : {}, defaultLocation: path.join(getConfigDir(), 'worktrees', '<name>'), insideIgnored,
		...defaultBranch ? {defaultBranch} : {}, ...originBranch ? {originBranch} : {}, user: userSlug(),
		agentHooks: user.agent_hooks === true, notifications: user.notifications === true, ...codexHooks ? {codexHooks} : {},
	};
}

export type CandidateKind = 'dir' | 'file' | 'symlink';
export interface WorktreeCandidate {
	/** Relative to the main checkout (the usual launch checkout); valid as a worktree.symlink entry. */
	path: string;
	kind: CandidateKind;
	/** Real path of a symlink entry (undefined when broken). */
	target?: string;
	/** Matched by .gitignore (otherwise untracked). */
	ignored: boolean;
	suggestion: 'link' | 'skip';
	reason: string;
	/** Already in a layer's symlink list or files destinations (as written, trusted or not), with the layers that list it. */
	configured?: 'symlink' | 'files';
	layers?: ConfigTargetKind[];
	/** A files entry's source. */
	source?: string;
	/** Configured but absent from the checkout. */
	missing?: boolean;
}
/** Settings → Linked items: the untracked/ignored entries of the main checkout plus every configured entry. */
export interface WorktreeCandidates {checkout: string; candidates: WorktreeCandidate[]; moreCandidates: number; candidatesError?: string}

const LINK_SEGMENTS = new Set(['node_modules', '.venv', 'venv', 'vendor', '.vercel']);
const SKIP_SEGMENTS: Record<string, string> = {dist: 'build output', build: 'build output', out: 'build output', coverage: 'coverage', '.next': 'build cache', '.turbo': 'build cache', '.cache': 'cache', __pycache__: 'cache', '.pytest_cache': 'cache', '.DS_Store': 'OS clutter'};
const isEnvName = (segment: string) => segment === '.env' || segment.startsWith('.env.') || segment.endsWith('.env');
/** Pure: whether an untracked/ignored entry is suggested as a link (dependencies, env files) or skipped (build/cache/clutter/everything else). */
export function classifyCandidate(relative: string): {suggestion: 'link' | 'skip'; reason: string} {
	const segments = relative.split('/');
	if (segments.some(segment => LINK_SEGMENTS.has(segment))) return {suggestion: 'link', reason: 'dependencies'};
	if (segments.some(isEnvName)) return {suggestion: 'link', reason: 'env file'};
	const skip = segments.find(segment => Object.hasOwn(SKIP_SEGMENTS, segment));
	if (skip) return {suggestion: 'skip', reason: SKIP_SEGMENTS[skip]!};
	if (segments.at(-1)!.endsWith('.log')) return {suggestion: 'skip', reason: 'log'};
	return {suggestion: 'skip', reason: ''};
}

const MAX_CANDIDATES = 200, MAX_LSTATS = 2000;
const firstLine = (error: unknown) => message(error).split('\n')[0]!;
async function entryKind(file: string): Promise<{kind: CandidateKind; target?: string} | undefined> {
	const stat = await fs.lstat(file).catch(() => undefined);
	if (!stat) return undefined;
	if (stat.isSymbolicLink()) return {kind: 'symlink', target: await fs.realpath(file).catch(() => undefined)};
	return {kind: stat.isDirectory() ? 'dir' : 'file'};
}
type Layers = Partial<Record<ConfigTargetKind, WorktreeSettings>>;
/** Untracked + ignored entries of `checkout` (directories collapsed), plus configured entries, configured first. */
async function readCandidates(checkout: string, layers: Layers): Promise<{candidates: WorktreeCandidate[]; more: number}> {
	const settings = mergeWorktreeSettings(layers.global, layers.repository) ?? {};
	const status = parseStatus(await git(checkout, ['--no-optional-locks', 'status', '--porcelain=v2', '-z', '--ignored=matching', '--untracked-files=normal'], {maxBuffer: 32 * 1024 * 1024}));
	const configured = new Map<string, Pick<WorktreeCandidate, 'configured' | 'layers' | 'source'>>();
	for (const entry of settings.symlink ?? []) configured.set(entry, {configured: 'symlink', layers: (['global', 'repository'] as const).filter(layer => layers[layer]?.symlink?.includes(entry))});
	for (const [destination, source] of Object.entries(settings.files ?? {})) configured.set(destination, {configured: 'files', source, layers: (['global', 'repository'] as const).filter(layer => layers[layer]?.files && Object.hasOwn(layers[layer]!.files!, destination))});
	const entries = [...status.untracked.map(file => ({file, ignored: false})), ...status.ignored.map(file => ({file, ignored: true}))]
		.map(entry => ({...entry, file: entry.file.replace(/\/$/, '')}))
		.filter(entry => validRelativePath(entry.file) && entry.file !== PROJECT_CONFIG_FILE && !configured.has(entry.file))
		.map(entry => ({...entry, ...classifyCandidate(entry.file)}))
		.sort((a, b) => Number(b.suggestion === 'link') - Number(a.suggestion === 'link') || a.file.localeCompare(b.file));
	const shown = entries.slice(0, MAX_CANDIDATES);
	const candidates: WorktreeCandidate[] = [];
	for (const [file, config] of configured) {
		const kind = await entryKind(path.join(checkout, file));
		candidates.push({path: file, kind: kind?.kind ?? 'file', ...kind?.target ? {target: kind.target} : {}, ignored: status.ignored.some(entry => entry.replace(/\/$/, '') === file), ...classifyCandidate(file), ...config, ...kind ? {} : {missing: true}});
	}
	let checks = 0;
	for (const entry of shown) {
		const kind = ++checks <= MAX_LSTATS ? await entryKind(path.join(checkout, entry.file)) : undefined;
		candidates.push({path: entry.file, kind: kind?.kind ?? 'file', ...kind?.target ? {target: kind.target} : {}, ignored: entry.ignored, suggestion: entry.suggestion, reason: entry.reason});
	}
	return {candidates, more: entries.length - shown.length};
}

export async function readWorktreeCandidates(cwd: string): Promise<WorktreeCandidates> {
	const context = await resolveRepoContext(cwd);
	const user = await loadAppConfig();
	const checkout = context.mainRoot ?? context.root;
	// Both layers as written, trusted or not: the picker edits files.
	const layers: Layers = {};
	try { layers.global = validateProjectConfig(user.defaults ?? {}, 'defaults').worktree; } catch {}
	try { layers.repository = (await loadProjectConfig(cwd, user, context)).config.worktree; } catch {}
	try {
		const read = await readCandidates(checkout, layers);
		return {checkout, candidates: read.candidates, moreCandidates: read.more};
	} catch (error) { return {checkout, candidates: [], moreCandidates: 0, candidatesError: firstLine(error)}; }
}

const MAX_SIZE_PATHS = 24, SIZE_TIMEOUT_MS = 4000, SIZE_CONCURRENCY = 4;
function du(file: string): Promise<number | null> {
	return new Promise(resolve => {
		const child = execFile('du', ['-sk', '--', file], {timeout: SIZE_TIMEOUT_MS, maxBuffer: 64 * 1024, encoding: 'utf8'}, (error, stdout) => {
			const kib = Number.parseInt(stdout.trim().split(/\s/)[0] ?? '', 10);
			// du exits non-zero for unreadable children but still prints a total; a timeout prints nothing.
			resolve(Number.isFinite(kib) && !(error && (error as {killed?: boolean}).killed) ? kib : null);
		});
		child.stdin?.end();
	});
}
/** Sizes in KiB for candidate paths (relative to the checkout); null when unknown or timed out. Symlinks report 0 (they hold no data). */
export async function readCandidateSizes(cwd: string, paths: string[]): Promise<Record<string, number | null>> {
	if (!Array.isArray(paths) || paths.length > MAX_SIZE_PATHS || !paths.every(validRelativePath)) throw new Error(`Pass at most ${MAX_SIZE_PATHS} relative candidate paths`);
	const {mainRoot, root} = await resolveRepoContext(cwd);
	const checkout = mainRoot ?? root;
	const sizes: Record<string, number | null> = {};
	const queue = [...paths];
	await Promise.all(Array.from({length: SIZE_CONCURRENCY}, async () => {
		for (let file = queue.shift(); file !== undefined; file = queue.shift()) {
			const absolute = path.join(checkout, file);
			const stat = await fs.lstat(absolute).catch(() => undefined);
			sizes[file] = !stat ? null : stat.isSymbolicLink() ? 0 : stat.isDirectory() ? await du(absolute) : Math.ceil(stat.size / 1024);
		}
	}));
	return sizes;
}
