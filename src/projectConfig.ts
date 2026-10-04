import fs from 'node:fs/promises';
import {constants} from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {resolveCreateScript, resolveRepoContext, type CreationHook, type RepoContext} from './git.js';
import type {ProgramKey, WorktreeMode} from './types.js';
import {MAX_CONFIG_BYTES} from './configDraft.js';
import {getConfigPath} from './paths.js';
import {mergeWorktreeSettings, validateWorktreeSettings, type WorktreeSettings} from './worktreeLinks.js';

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
