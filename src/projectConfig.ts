import fs from 'node:fs/promises';
import {constants} from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {resolveCreateScript, resolveRepoContext, worktreeTemplateVars, type CreationHook, type RepoContext} from './git.js';
import type {ProgramKey, WorktreeMode} from './types.js';
import {MAX_CONFIG_BYTES} from './configDraft.js';
import {getConfigDir, getConfigPath} from './paths.js';
import {expandBranchName, expandWorktreeTemplate, mergeWorktreeSettings, userSlug, validateWorktreeSettings, worktreeLocation, type TemplateVars, type WorktreeSettings} from './worktreeLinks.js';

export const PROJECT_CONFIG_FILE = 'deckhand.json';
const MAX_TRUSTED_FINGERPRINTS = 20;
export function sha256(data: string | Uint8Array): string { return createHash('sha256').update(data).digest('hex'); }

/** Reads at most `max` bytes as strict UTF-8 (BOM kept, so text round-trips to the hashed bytes). Non-blocking open avoids FIFO hangs. */
export async function readBoundedUtf8(file: string, {noFollow = false, max}: {noFollow?: boolean; max: number}): Promise<{bytes: Buffer; text: string; mode: number}> {
	const name = path.basename(file), limit = `${name} exceeds ${Math.round(max / 1024)} KiB`;
	let handle: Awaited<ReturnType<typeof fs.open>> | undefined;
	try {
		handle = await fs.open(file, constants.O_RDONLY | constants.O_NONBLOCK | (noFollow ? constants.O_NOFOLLOW : 0));
		const stat = await handle.stat();
		if (!stat.isFile()) throw new Error(`${name} must be a regular file`);
		if (stat.size > max) throw new Error(limit);
		const buffer = Buffer.alloc(max + 1);
		let length = 0;
		while (length < buffer.length) {
			const {bytesRead} = await handle.read(buffer, length, buffer.length - length, null);
			if (!bytesRead) break;
			length += bytesRead;
		}
		if (length > max) throw new Error(limit);
		const bytes = buffer.subarray(0, length);
		return {bytes, text: new TextDecoder('utf-8', {fatal: true, ignoreBOM: true}).decode(bytes), mode: stat.mode & 0o777};
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === 'ELOOP' && noFollow) throw Object.assign(new Error(`${name} must not be a symlink`), {code: 'ELOOP'});
		throw error;
	} finally { await handle?.close(); }
}

export interface ProjectConfig {
	defaultAgent?: ProgramKey;
	defaultWorkspace?: WorktreeMode;
	devCommand?: string;
	setupCommand?: string;
	actions?: Record<string, string>;
	worktree?: WorktreeSettings;
}
/** The repository override (the main checkout's deckhand.json) plus the creation hook, which are trusted together. */
export interface LoadedProject {
	/** Git root of the worktree the project was resolved from; the creation hook is looked up from here. */
	root: string;
	trustRoot: string;
	/** The main checkout's deckhand.json; undefined for bare repositories, which have no repository override. */
	path?: string;
	config: ProjectConfig;
	exists: boolean;
	fingerprint: string;
	/** The enabled creation hook: read, fingerprinted and reviewed; it runs only while trusted. */
	creationHook?: CreationHook;
	/** A detected create-worktree.sh that `worktree.hook: false` disables: never read, run, fingerprinted or reviewed. */
	disabledHook?: {file: string; by: 'global' | 'repository'};
}
/** Trusted fingerprints per trust root, newest first. */
export interface ProjectTrust {trustedProjects?: Record<string, string[]>}
/** The parts of the user config that feed effective settings. `defaults` is validated where it is used. */
export interface UserSettings extends ProjectTrust {dev_command?: string; defaults?: unknown}

/** Validates a deckhand.json-shaped object (the repository file or the user config's `defaults`). */
export function validateProjectConfig(value: unknown, label = 'deckhand.json'): ProjectConfig {
	if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${label} must be an object`);
	const record = value as Record<string, unknown>;
	const allowed = new Set(['defaultAgent', 'defaultWorkspace', 'devCommand', 'setupCommand', 'actions', 'worktree']);
	for (const key of Object.keys(record)) if (!allowed.has(key)) throw new Error(`Unknown ${label} setting: ${key}`);
	if (record.defaultAgent !== undefined && (typeof record.defaultAgent !== 'string' || !['claude', 'pi', 'codex'].includes(record.defaultAgent))) throw new Error('Invalid defaultAgent');
	if (record.defaultWorkspace !== undefined && (typeof record.defaultWorkspace !== 'string' || !['none', 'new', 'existing'].includes(record.defaultWorkspace))) throw new Error('Invalid defaultWorkspace');
	const command = (value: unknown, label: string) => {
		if (typeof value !== 'string' || !value.trim() || value.length > 8192 || value.includes('\0')) throw new Error(`Invalid command: ${label}`);
	};
	for (const key of ['devCommand', 'setupCommand']) if (record[key] !== undefined) command(record[key], key);
	if (record.actions !== undefined) {
		if (!record.actions || typeof record.actions !== 'object' || Array.isArray(record.actions)) throw new Error('actions must be a command map');
		const entries = Object.entries(record.actions);
		if (entries.length > 30) throw new Error('At most 30 actions are supported');
		for (const [name, value] of entries) {
			if (!/^[a-zA-Z0-9][a-zA-Z0-9 _.-]{0,47}$/.test(name) || ['__proto__', 'constructor', 'prototype'].includes(name)) throw new Error(`Invalid action name: ${name}`);
			command(value, name);
		}
	}
	if (record.worktree !== undefined) validateWorktreeSettings(record.worktree);
	return record as ProjectConfig;
}
export function parseProjectConfig(raw: string, label?: string): ProjectConfig {
	return validateProjectConfig(JSON.parse(raw.replace(/^\uFEFF/, '')), label);
}

/**
 * Whether a detected creation hook is enabled. `worktree.hook: false` in the repository file is honoured even while
 * that file is untrusted: it can only stop a script from running, never make anything run. Enabling never bypasses
 * trust: an enabled hook is part of the fingerprint and runs only while trusted, so a repository `hook: true` that
 * overrides a global `false` takes effect only once the override (with the hook) is trusted. Global defaults are
 * read leniently here (invalid defaults are reported where they are used), again because false only disables.
 */
function hookSetting(config: ProjectConfig, user: UserSettings): {enabled: boolean; by?: 'global' | 'repository'} {
	const repository = config.worktree?.hook;
	if (repository !== undefined) return repository ? {enabled: true} : {enabled: false, by: 'repository'};
	const defaults = user.defaults as {worktree?: {hook?: unknown}} | undefined;
	return defaults?.worktree?.hook === false ? {enabled: false, by: 'global'} : {enabled: true};
}

/**
 * Loads the repository override for `cwd`: always the main checkout's live deckhand.json; copies inside linked
 * worktrees are ignored. `user` is the user config: its `defaults.worktree.hook` can disable the creation hook,
 * which changes the fingerprint, so every caller that compares fingerprints must pass the same user config.
 */
export async function loadProjectConfig(cwd: string, user: UserSettings = {}, context?: RepoContext): Promise<LoadedProject> {
	const {root, mainRoot, trustRoot} = context ?? await resolveRepoContext(cwd);
	const file = mainRoot ? path.join(mainRoot, PROJECT_CONFIG_FILE) : undefined;
	let raw = '';
	let exists = false;
	if (file) {
		try {
			raw = (await readBoundedUtf8(file, {noFollow: true, max: MAX_CONFIG_BYTES})).text;
			exists = true;
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw new Error(`${file}: ${error instanceof Error ? error.message : String(error)}`);
		}
	}
	let config: ProjectConfig;
	try { config = exists ? parseProjectConfig(raw) : {}; }
	catch (error) { throw new Error(`${file}: ${error instanceof Error ? error.message : String(error)}`); }
	const hookPath = await resolveCreateScript(root, mainRoot);
	let creationHook: LoadedProject['creationHook'];
	let disabledHook: LoadedProject['disabledHook'];
	if (hookPath) {
		const setting = hookSetting(config, user);
		if (!setting.enabled) disabledHook = {file: hookPath, by: setting.by!};
		else {
			const {bytes, text} = await readBoundedUtf8(hookPath, {max: MAX_CONFIG_BYTES});
			creationHook = {file: hookPath, content: text, fingerprint: sha256(bytes)};
		}
	}
	const fingerprint = sha256(`config\0${raw}\0creation-hook\0${creationHook?.fingerprint ?? ''}`);
	return {root, trustRoot, path: file, config, exists, fingerprint, ...(creationHook ? {creationHook} : {}), ...(disabledHook ? {disabledHook} : {})};
}

export function isProjectTrusted(project: LoadedProject, config: ProjectTrust): boolean {
	const trusted = config.trustedProjects?.[project.trustRoot];
	return Array.isArray(trusted) && trusted.includes(project.fingerprint);
}
/** Only a repository with a deckhand.json or an enabled creation hook has anything to trust. */
export function projectNeedsReview(project: LoadedProject, config: ProjectTrust): boolean {
	return (project.exists || Boolean(project.creationHook)) && !isProjectTrusted(project, config);
}
/** Prepends the project's fingerprint for its trust root (deduped, capped); use as an updateAppConfig updater. */
export function trustProjectConfig<T extends object>(project: LoadedProject, config: T & ProjectTrust): T & Required<ProjectTrust> {
	const previous = config.trustedProjects?.[project.trustRoot];
	const list = [project.fingerprint, ...(Array.isArray(previous) ? previous : []).filter(value => value !== project.fingerprint)].slice(0, MAX_TRUSTED_FINGERPRINTS);
	return {...config, trustedProjects: {...config.trustedProjects, [project.trustRoot]: list}};
}

/** The user's global defaults (config.json `defaults`, then the legacy `dev_command`). User-authored, so never trust-gated. */
export function globalDefaults(user: UserSettings): ProjectConfig {
	let defaults: ProjectConfig = {};
	if (user.defaults !== undefined) {
		try { defaults = validateProjectConfig(user.defaults, 'defaults'); }
		catch (error) { throw new Error(`Invalid "defaults" in ${getConfigPath()}: ${error instanceof Error ? error.message : String(error)}. Fix it with C → Global defaults.`); }
	}
	const legacyDev = user.dev_command?.trim();
	return defaults.devCommand === undefined && legacyDev ? {...defaults, devCommand: legacyDev} : defaults;
}
/**
 * Effective settings: global defaults overlaid field by field by the repository override, only while that
 * override is trusted (untrusted or absent → global only). Actions merge by name, the repository winning;
 * worktree settings merge per field (see mergeWorktreeSettings).
 */
export function resolveSettings(project: LoadedProject | undefined, user: UserSettings): ProjectConfig {
	const global = globalDefaults(user);
	const repository = project && isProjectTrusted(project, user) ? project.config : {};
	const merged: ProjectConfig = {...global, ...repository};
	if (global.actions || repository.actions) merged.actions = {...global.actions, ...repository.actions};
	const worktree = mergeWorktreeSettings(global.worktree, repository.worktree);
	if (worktree) merged.worktree = worktree;
	// An untrusted repository may still switch the creation hook off (see hookSetting); it can never switch it on.
	if (project?.config.worktree?.hook === false) merged.worktree = {...merged.worktree, hook: false};
	return merged;
}
export function resolveDevCommand(project: LoadedProject | undefined, user: UserSettings): string {
	return resolveSettings(project, user).devCommand?.trim() || 'dev';
}
/**
 * Setup for a new worktree. An untrusted repository setupCommand never runs: it is ignored when the user
 * reviewed exactly these bytes and chose to continue without trust (`reviewedFingerprint`), and refused
 * otherwise (not reviewed, or the file changed after review) so the session reports it.
 */
export function resolveSetupCommand(project: LoadedProject | undefined, user: UserSettings, reviewedFingerprint?: string): string | undefined {
	const settings = resolveSettings(project, user);
	if (project?.config.setupCommand && !isProjectTrusted(project, user) && reviewedFingerprint !== project.fingerprint) {
		throw new Error('Repository deckhand.json is not trusted (unreviewed or changed since review); setup has not run. Press s to review it and retry');
	}
	return settings.setupCommand;
}

/** Where an effective setting comes from (C → Effective settings). */
export type SettingSource = 'repo' | 'global' | 'legacy dev_command' | 'built-in default' | 'not set';
/**
 * One row of the effective-settings breakdown. List-valued settings (actions, worktree.symlink, worktree.files) get
 * one row per entry. `raw` is the winning layer's value as written (templates unexpanded), absent for built-in
 * defaults and unset rows; `value` is display-ready (templates expanded). `pending` is what an untrusted repository
 * file would set here once trusted. Rows with source 'not set' and a `pending` exist only in the untrusted file.
 */
export interface SettingRow {key: string; entry?: string; value: string; source: SettingSource; raw?: string | boolean; pending?: {value: string; raw: string | boolean}; note?: string}
export interface ExplainContext {
	/** Template placeholders (name `<name>`) for expanding location/files; without them templates are shown as written. */
	vars?: TemplateVars;
	/** `{user}` for the branch-name example. */
	user?: string;
	/** A detected create-worktree.sh when the project could not be loaded (otherwise taken from the project). */
	hookFile?: string;
	/** Deckhand's state directory (default getConfigDir()), for the built-in worktree location. */
	configDir?: string;
}
export interface SettingsExplanation {rows: SettingRow[]; globalError?: string}
const SCALARS = [['defaultAgent', 'claude'], ['defaultWorkspace', 'none'], ['devCommand', 'dev'], ['setupCommand', undefined]] as const;

/**
 * The effective settings row by row with their sources: values come from resolveSettings (invalid global defaults
 * are reported and skipped, keeping the legacy dev_command), sources from the layers, pending values from an
 * untrusted repository file. Pure; never reads files.
 */
export function explainSettings(project: LoadedProject | undefined, user: UserSettings, context: ExplainContext = {}): SettingsExplanation {
	let global: ProjectConfig, globalError: string | undefined, effective: ProjectConfig;
	try { global = globalDefaults(user); effective = resolveSettings(project, user); }
	catch (error) {
		globalError = error instanceof Error ? error.message : String(error);
		const lenient = {...user, defaults: undefined};
		global = globalDefaults(lenient); effective = resolveSettings(project, lenient);
	}
	const trusted = Boolean(project && isProjectTrusted(project, user));
	const repository: ProjectConfig = project && trusted ? project.config : {};
	const pendingLayer: ProjectConfig = project && !trusted ? project.config : {};
	const {vars} = context;
	const location = (template: string) => { if (!vars) return template; try { return worktreeLocation(template, vars); } catch { return template; } };
	const source = (template: string) => { if (!vars) return template; try { return expandWorktreeTemplate(template, vars); } catch { return template; } };
	const branch = (template: string) => { try { return expandBranchName(template, {name: vars?.name ?? '<name>', user: context.user ?? 'user'}); } catch { return template; } };
	const show = (value: string | boolean) => typeof value === 'boolean' ? (value ? 'on' : 'off') : value;
	const rows: SettingRow[] = [];
	const hookFile = project ? project.creationHook?.file ?? project.disabledHook?.file : context.hookFile;
	const hookActive = trusted && Boolean(project?.creationHook);
	const overrides = (globalValue: unknown, value: unknown, display: (value: string | boolean) => string) => globalValue !== undefined && globalValue !== value ? `overrides global ${display(globalValue as string | boolean)}` : undefined;
	const pick = <T extends string | boolean>(key: string, value: T | undefined, layers: {repo?: T; global?: T; pending?: T}, fallback: {value?: T; display: (value: T) => string}, extra: {legacy?: boolean; note?: string} = {}) => {
		const display = fallback.display as (value: string | boolean) => string;
		const from: SettingSource = value === undefined ? (fallback.value === undefined ? 'not set' : 'built-in default') : layers.repo !== undefined && layers.repo === value ? 'repo' : extra.legacy ? 'legacy dev_command' : 'global';
		const row: SettingRow = {key, value: value !== undefined ? display(value) : fallback.value !== undefined ? display(fallback.value) : '—', source: from, ...value !== undefined ? {raw: value} : {}};
		const note = [from === 'repo' ? overrides(layers.global, value, display) : undefined, extra.note].filter(Boolean).join(' · ');
		if (note) row.note = note;
		if (layers.pending !== undefined) row.pending = {value: display(layers.pending), raw: layers.pending};
		rows.push(row);
	};
	for (const [key, builtIn] of SCALARS) {
		pick<string>(key, effective[key], {repo: repository[key], global: global[key], pending: pendingLayer[key]}, {value: builtIn, display: String}, {legacy: key === 'devCommand' && repository.devCommand === undefined && effective.devCommand !== undefined && (globalError !== undefined || (user.defaults as ProjectConfig | undefined)?.devCommand === undefined)});
	}
	const entries = (key: string, values: Record<string, string> | undefined, layers: {repo?: Record<string, string>; global?: Record<string, string>; pending?: Record<string, string>}, display: (value: string) => string) => {
		const before = rows.length;
		for (const [entry, value] of Object.entries(values ?? {})) {
			const repo = layers.repo !== undefined && Object.hasOwn(layers.repo, entry);
			const row: SettingRow = {key, entry, value: display(value), source: repo ? 'repo' : 'global', raw: value};
			const globalValue = layers.global && Object.hasOwn(layers.global, entry) ? layers.global[entry] : undefined;
			if (repo && globalValue !== undefined && globalValue !== value) row.note = `overrides global ${display(globalValue)}`;
			if (layers.pending && Object.hasOwn(layers.pending, entry)) row.pending = {value: display(layers.pending[entry]!), raw: layers.pending[entry]!};
			rows.push(row);
		}
		for (const [entry, value] of Object.entries(layers.pending ?? {})) if (!values || !Object.hasOwn(values, entry)) rows.push({key, entry, value: '—', source: 'not set', pending: {value: display(value), raw: value}});
		if (rows.length === before) rows.push({key, value: '—', source: 'not set'});
	};
	entries('actions', effective.actions, {repo: repository.actions, global: global.actions, pending: pendingLayer.actions}, String);
	const tree = effective.worktree ?? {}, repoTree = repository.worktree ?? {}, globalTree = global.worktree ?? {}, pendingTree = pendingLayer.worktree ?? {};
	const ignored = hookActive ? 'ignored: the creation hook decides' : undefined;
	pick<string>('worktree.location', tree.location, {repo: repoTree.location, global: globalTree.location, pending: pendingTree.location}, {value: path.join(context.configDir ?? getConfigDir(), 'worktrees', vars?.name ?? '{name}'), display: location}, {note: ignored});
	pick<string>('worktree.branchFrom', tree.branchFrom, {repo: repoTree.branchFrom, global: globalTree.branchFrom, pending: pendingTree.branchFrom}, {value: 'current', display: String}, {note: ignored});
	pick<string>('worktree.branchName', tree.branchName, {repo: repoTree.branchName, global: globalTree.branchName, pending: pendingTree.branchName}, {value: '{name}', display: branch}, {note: ignored});
	// The repository's hook: false applies even untrusted (see hookSetting); only `true` can be pending.
	const hookOn = tree.hook !== false;
	const hookNote = !hookFile ? 'no .claude/scripts/create-worktree.sh detected' : !hookOn ? '.claude/scripts/create-worktree.sh detected, not used' : hookActive ? '.claude/scripts/create-worktree.sh detected: decides location and branch' : '.claude/scripts/create-worktree.sh detected: will not run until trusted (T)';
	pick<boolean>('worktree.hook', tree.hook, {repo: project?.config.worktree?.hook === false ? false : repoTree.hook, global: globalTree.hook, pending: pendingTree.hook === true ? true : undefined}, {value: true, display: show}, {note: hookNote});
	const symlinks = tree.symlink ?? [], repoLinks = new Set(repoTree.symlink ?? []), globalLinks = new Set(globalTree.symlink ?? []);
	const before = rows.length;
	for (const entry of symlinks) rows.push({key: 'worktree.symlink', entry, value: entry, source: globalLinks.has(entry) ? 'global' : repoLinks.has(entry) ? 'repo' : 'global', raw: entry});
	for (const entry of pendingTree.symlink ?? []) if (!symlinks.includes(entry)) rows.push({key: 'worktree.symlink', entry, value: '—', source: 'not set', pending: {value: entry, raw: entry}});
	if (rows.length === before) rows.push({key: 'worktree.symlink', value: '—', source: 'not set'});
	entries('worktree.files', tree.files, {repo: repoTree.files, global: globalTree.files, pending: pendingTree.files}, source);
	return {rows, ...globalError ? {globalError} : {}};
}

/** C → Effective settings for `cwd`'s repository: the trust state of its deckhand.json plus the breakdown. Read-only. */
export interface EffectiveSettingsInfo {
	cwd: string;
	repo: string;
	repository: {state: 'trusted' | 'untrusted' | 'absent' | 'invalid' | 'bare' | 'none'; path?: string; error?: string};
	/** T has something to review (an untrusted deckhand.json or enabled creation hook). */
	needsReview: boolean;
	globalError?: string;
	rows: SettingRow[];
}
export async function readEffectiveSettings(cwd: string, user: UserSettings): Promise<EffectiveSettingsInfo> {
	const message = (error: unknown) => error instanceof Error ? error.message : String(error);
	let context: RepoContext | undefined, project: LoadedProject | undefined, error: string | undefined;
	try { context = await resolveRepoContext(cwd); } catch (caught) { error = `Not a Git repository: ${message(caught)}`; }
	if (context) try { project = await loadProjectConfig(cwd, user, context); } catch (caught) { error = message(caught); }
	const hookFile = context && !project ? await resolveCreateScript(context.root, context.mainRoot).catch(() => undefined) : undefined;
	const vars = context ? await worktreeTemplateVars('<name>', cwd).catch(() => undefined) : undefined;
	const {rows, globalError} = explainSettings(project, user, {...vars ? {vars} : {}, user: userSlug(), ...hookFile ? {hookFile} : {}});
	const state: EffectiveSettingsInfo['repository']['state'] = !context ? 'none' : error ? 'invalid' : !project?.path ? 'bare' : !project.exists ? 'absent' : isProjectTrusted(project, user) ? 'trusted' : 'untrusted';
	const file = project?.path ?? (context?.mainRoot ? path.join(context.mainRoot, PROJECT_CONFIG_FILE) : undefined);
	return {cwd, repo: vars?.repo ?? path.basename(cwd), repository: {state, ...file ? {path: file} : {}, ...error ? {error} : {}}, needsReview: Boolean(project && projectNeedsReview(project, user)), ...globalError ? {globalError} : {}, rows};
}
