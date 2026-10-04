import fs from 'node:fs/promises';
import path from 'node:path';
import {execFile} from 'node:child_process';
import {git, optionalGit, resolveCreateScript, resolveDefaultBranch, resolveRepoContext, worktreeTemplateVars} from './git.js';
import {isProjectTrusted, loadProjectConfig, projectNeedsReview, validateProjectConfig, PROJECT_CONFIG_FILE} from './projectConfig.js';
import {readConfigTargets, type ConfigTargetKind, type ConfigTargets, type ProjectConfigDocument} from './projectConfigDocument.js';
import {parseStatus} from './workspaceGit.js';
import {getConfigDir} from './paths.js';
import {loadAppConfig} from './storage.js';
import {expandBranchName, mergeWorktreeSettings, userSlug, validRelativePath, validateWorktreeSettings, worktreeLocation, type BranchFrom, type TemplateVars, type WorktreeSettings} from './worktreeLinks.js';

// The Worktree setup screen (C → Worktree setup): pure model helpers used by the UI, plus the daemon-side
// readers (candidates from the main checkout, bounded sizes). Saving goes through the regular save-config path.

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
	/** Already in the effective symlink list or a files destination, with the layers that list it. */
	configured?: 'symlink' | 'files';
	layers?: ConfigTargetKind[];
	/** A files entry's source. */
	source?: string;
	/** Configured but absent from the checkout. */
	missing?: boolean;
}
export const LOCATION_PRESETS = {next: '{repoParent}/worktrees/{name}', inside: '{repoRoot}/.worktrees/{name}'} as const;
export type LocationPreset = 'next' | 'default' | 'inside' | 'custom';
export interface WorktreeSetupInfo {
	cwd: string;
	repo: string;
	/** Where candidates were read: the main checkout (or the launch checkout of a bare repository). */
	checkout: string;
	hook?: {file: string; enabled: boolean; by?: 'global' | 'repository'; trusted: boolean};
	repositoryTrusted: boolean;
	repositoryNeedsReview: boolean;
	layers: {global?: WorktreeSettings; repository?: WorktreeSettings};
	layerErrors: {global?: string; repository?: string};
	targets: ConfigTargets;
	candidates: WorktreeCandidate[];
	moreCandidates: number;
	candidatesError?: string;
	/** Template placeholders with `<name>` for previews. */
	vars: TemplateVars;
	defaultLocation: string;
	insideIgnored: boolean;
	defaultBranch?: string;
	/** origin's default branch name when an origin remote exists. */
	originBranch?: string;
	user: string;
}

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

export interface WorktreeSetupModel {
	target: ConfigTargetKind;
	location: LocationPreset;
	/** A location that matches no preset, kept so it can be chosen again. */
	customLocation?: string;
	branchFrom: BranchFrom;
	branchName: string;
	hook: boolean;
	/** Candidate path → linked. */
	links: Record<string, boolean>;
}
/** The effective-if-trusted worktree settings: both layers as written, regardless of trust (the screen edits files). */
export function setupSettings(info: WorktreeSetupInfo): WorktreeSettings {
	const merged = mergeWorktreeSettings(info.layers.global, info.layers.repository) ?? {};
	return info.layers.repository?.hook === false ? {...merged, hook: false} : merged;
}
export function locationPreset(location: string | undefined): LocationPreset {
	if (!location) return 'default';
	return location === LOCATION_PRESETS.next ? 'next' : location === LOCATION_PRESETS.inside ? 'inside' : 'custom';
}
export function initialSetupModel(info: WorktreeSetupInfo): WorktreeSetupModel {
	const settings = setupSettings(info);
	const location = locationPreset(settings.location);
	return {
		target: info.targets.repository ? 'repository' : 'global',
		location, ...(location === 'custom' ? {customLocation: settings.location} : {}),
		branchFrom: settings.branchFrom ?? 'current', branchName: settings.branchName ?? '{name}', hook: settings.hook ?? true,
		links: Object.fromEntries(info.candidates.map(candidate => [candidate.path, candidate.configured ? true : candidate.suggestion === 'link'])),
	};
}
/** Whether the model differs from what was loaded (the save target alone is not an edit). */
export function setupModelDirty(model: WorktreeSetupModel, initial: WorktreeSetupModel): boolean {
	const strip = ({target: _target, ...rest}: WorktreeSetupModel) => JSON.stringify(rest);
	return strip(model) !== strip(initial);
}
export function locationTemplate(model: WorktreeSetupModel): string | undefined {
	return model.location === 'default' ? undefined : model.location === 'custom' ? model.customLocation : LOCATION_PRESETS[model.location];
}
/**
 * Pure: the `worktree` section to write into the model's target. Location (unless "default", which writes no
 * location), branch settings and hook are written whenever they differ from what the target would otherwise
 * inherit, symlink is exactly the linked candidates (files rows excluded), and the target's own `files` stay untouched.
 */
export function worktreeSection(model: WorktreeSetupModel, info: WorktreeSetupInfo): WorktreeSettings {
	const own = info.layers[model.target] ?? {};
	const inherited = model.target === 'repository' ? info.layers.global ?? {} : {};
	const section: WorktreeSettings = {};
	const location = locationTemplate(model);
	if (location) section.location = location;
	const symlink = info.candidates.filter(candidate => candidate.configured !== 'files' && model.links[candidate.path]).map(candidate => candidate.path);
	if (symlink.length) section.symlink = symlink;
	if (own.files) section.files = own.files;
	if (model.branchFrom !== (inherited.branchFrom ?? 'current')) section.branchFrom = model.branchFrom;
	if (model.branchName !== (inherited.branchName ?? '{name}')) section.branchName = model.branchName;
	if (model.hook !== (inherited.hook ?? true)) section.hook = model.hook;
	return validateWorktreeSettings(section);
}
/** Pure: `document`'s JSON with its `worktree` replaced by `section` (removed when empty); every other key is kept in place. */
export function applyWorktreeSection(document: Pick<ProjectConfigDocument, 'raw' | 'exists' | 'kind'>, section: WorktreeSettings): string {
	let value: unknown;
	try { value = document.exists ? JSON.parse(document.raw.replace(/^﻿/, '')) : {}; }
	catch { throw new Error(`${document.kind === 'global' ? 'Global defaults are' : 'deckhand.json is'} not valid JSON; press e to repair it first`); }
	if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('The configuration is not a JSON object; press e to repair it first');
	const next: Record<string, unknown> = {...value as Record<string, unknown>};
	if (Object.keys(section).length) next.worktree = section; else delete next.worktree;
	const raw = `${JSON.stringify(next, null, 2)}\n`;
	validateProjectConfig(next, document.kind === 'global' ? 'defaults' : undefined);
	return raw;
}
/** The branch-name preview for `my-task`, or the reason the template is invalid. */
export function branchNameExample(template: string, user: string): {example?: string; error?: string} {
	try { validateWorktreeSettings({branchName: template}); return {example: expandBranchName(template, {name: 'my-task', user})}; }
	catch (error) { return {error: error instanceof Error ? error.message : String(error)}; }
}
export function previewLocation(template: string, vars: TemplateVars): string {
	try { return worktreeLocation(template, vars); } catch (error) { return `invalid: ${error instanceof Error ? error.message : String(error)}`; }
}

const MAX_CANDIDATES = 200, MAX_LSTATS = 2000;
const message = (error: unknown) => error instanceof Error ? error.message.split('\n')[0]! : String(error);
async function entryKind(file: string): Promise<{kind: CandidateKind; target?: string} | undefined> {
	const stat = await fs.lstat(file).catch(() => undefined);
	if (!stat) return undefined;
	if (stat.isSymbolicLink()) return {kind: 'symlink', target: await fs.realpath(file).catch(() => undefined)};
	return {kind: stat.isDirectory() ? 'dir' : 'file'};
}
/** Untracked + ignored entries of `checkout` (directories collapsed), plus configured entries, configured first. */
async function readCandidates(checkout: string, settings: WorktreeSettings, layers: WorktreeSetupInfo['layers']): Promise<{candidates: WorktreeCandidate[]; more: number}> {
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

export async function readWorktreeSetupInfo(cwd: string): Promise<WorktreeSetupInfo> {
	const context = await resolveRepoContext(cwd);
	const user = await loadAppConfig();
	const checkout = context.mainRoot ?? context.root;
	const layers: WorktreeSetupInfo['layers'] = {}, layerErrors: WorktreeSetupInfo['layerErrors'] = {};
	try { layers.global = validateProjectConfig(user.defaults ?? {}, 'defaults').worktree; } catch (error) { layerErrors.global = message(error); }
	let project: Awaited<ReturnType<typeof loadProjectConfig>> | undefined;
	try { project = await loadProjectConfig(cwd, user, context); layers.repository = project.config.worktree; } catch (error) { layerErrors.repository = message(error); }
	const hookFile = project?.creationHook?.file ?? project?.disabledHook?.file ?? (project ? undefined : await resolveCreateScript(context.root, context.mainRoot));
	const vars = await worktreeTemplateVars('<name>', cwd);
	const [targets, read, insideIgnored, defaultBranch, remotes] = await Promise.all([
		readConfigTargets(cwd),
		readCandidates(checkout, mergeWorktreeSettings(layers.global, layers.repository) ?? {}, layers).catch(error => ({candidates: [], more: 0, error: message(error)})),
		context.mainRoot ? optionalGit(context.mainRoot, ['check-ignore', '-q', '--', '.worktrees/probe']).then(result => result !== undefined) : Promise.resolve(false),
		resolveDefaultBranch(checkout),
		optionalGit(checkout, ['remote']),
	]);
	const originBranch = remotes?.split('\n').includes('origin') ? await resolveDefaultBranch(checkout, 'origin', 'remote') : undefined;
	return {
		cwd, repo: vars.repo, checkout,
		...hookFile ? {hook: {file: hookFile, enabled: !project?.disabledHook, ...project?.disabledHook ? {by: project.disabledHook.by} : {}, trusted: Boolean(project && isProjectTrusted(project, user))}} : {},
		repositoryTrusted: Boolean(project && isProjectTrusted(project, user)), repositoryNeedsReview: Boolean(project && projectNeedsReview(project, user)),
		layers, layerErrors, targets,
		candidates: read.candidates, moreCandidates: read.more, ...'error' in read ? {candidatesError: read.error} : {},
		vars, defaultLocation: path.join(getConfigDir(), 'worktrees', '<name>'), insideIgnored,
		...defaultBranch ? {defaultBranch} : {}, ...originBranch ? {originBranch} : {}, user: userSlug(),
	};
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
