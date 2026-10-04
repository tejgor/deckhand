import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {randomUUID} from 'node:crypto';

/**
 * Declarative worktree settings (the `worktree` section of global defaults or deckhand.json):
 * where new worktrees go and which heavy directories / private files are symlinked into them.
 */
export interface WorktreeSettings {
	/** Template for a new worktree's path; placeholders below, leading `~/` is the home directory. */
	location?: string;
	/** Paths relative to the launch checkout, linked into the new worktree at the same relative path. */
	symlink?: string[];
	/** Destination inside the new worktree → source (absolute/templated, or relative to the launch checkout). */
	files?: Record<string, string>;
	/** Start point of a new branch: the launch checkout's HEAD (default), the local default branch, or a freshly fetched origin default branch. */
	branchFrom?: BranchFrom;
	/** Template for a new branch's name: `{name}` (required) and `{user}`; default `{name}`. */
	branchName?: string;
	/** false: ignore .claude/scripts/create-worktree.sh entirely (never run, reviewed or fingerprinted). Default true. */
	hook?: boolean;
}
export const BRANCH_FROM = ['current', 'default', 'origin'] as const;
export type BranchFrom = typeof BRANCH_FROM[number];
export interface TemplateVars {name: string; repo: string; repoParent: string; repoRoot: string; home: string}
export interface LinkResult {linked: string[]; notes: string[]}

const PLACEHOLDERS = new Set(['name', 'repo', 'repoParent', 'repoRoot', 'home']);
const MAX_ENTRIES = 50;
const MAX_PATH = 4096;

/** Pure: expands `{placeholder}`s (unknown ones throw) and a leading `~/`. The result is not resolved. */
export function expandWorktreeTemplate(template: string, vars: TemplateVars): string {
	if (/[{}]/.test(template.replace(/\{[^{}]*\}/g, ''))) throw new Error(`Unbalanced braces in ${JSON.stringify(template)}`);
	const expanded = template.replace(/\{([^{}]*)\}/g, (_match, key: string) => {
		if (!PLACEHOLDERS.has(key)) throw new Error(`Unknown placeholder {${key}} in ${JSON.stringify(template)}`);
		return vars[key as keyof TemplateVars];
	});
	return expanded === '~' ? vars.home : expanded.startsWith('~/') ? path.join(vars.home, expanded.slice(2)) : expanded;
}
/** The absolute path of a new worktree for `vars` (the template must expand to an absolute path). */
export function worktreeLocation(template: string, vars: TemplateVars): string {
	const expanded = expandWorktreeTemplate(template, vars);
	if (!path.isAbsolute(expanded)) throw new Error(`worktree.location must expand to an absolute path: ${expanded}`);
	return path.resolve(expanded);
}

export interface BranchVars {name: string; user: string}
/** Pure: expands `{name}`/`{user}` in a branch-name template (unknown placeholders throw). */
export function expandBranchName(template: string, vars: BranchVars): string {
	if (/[{}]/.test(template.replace(/\{[^{}]*\}/g, ''))) throw new Error(`Unbalanced braces in ${JSON.stringify(template)}`);
	return template.replace(/\{([^{}]*)\}/g, (_match, key: string) => {
		if (key !== 'name' && key !== 'user') throw new Error(`Unknown placeholder {${key}} in branch name ${JSON.stringify(template)}`);
		return vars[key];
	});
}
/** A static approximation of `git check-ref-format --branch` (creation still asks Git); undefined when plausible. */
export function branchNameProblem(name: string): string | undefined {
	if (!name || name.length > 200) return 'empty or too long';
	if (/[\0-\x20\x7f~^:?*[\\]/.test(name)) return 'contains spaces, control characters or one of ~ ^ : ? * [ \\';
	if (name.includes('..') || name.includes('@{') || name.includes('//') || name === '@') return 'contains "..", "@{" or "//"';
	if (/^[-/.]|[/.]$|\.lock$|\/\.|\.lock\//.test(name)) return 'starts with -, / or ., ends with / or ., or has a component starting with . or ending in .lock';
	return undefined;
}
/** The OS username as a branch-safe slug (for `{user}`). */
export function userSlug(): string {
	let name = '';
	try { name = os.userInfo().username; } catch {}
	return name.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^[-.]+|[-.]+$/g, '').replace(/\.{2,}/g, '.') || 'user';
}

const DUMMY: TemplateVars = {name: 'name', repo: 'repo', repoParent: '/parent', repoRoot: '/parent/repo', home: '/home'};
function templateString(value: unknown, label: string): string {
	if (typeof value !== 'string' || !value.trim() || value.length > MAX_PATH || value.includes('\0')) throw new Error(`Invalid ${label}`);
	try { expandWorktreeTemplate(value, DUMMY); } catch (error) { throw new Error(`Invalid ${label}: ${error instanceof Error ? error.message : String(error)}`); }
	return value;
}
/** A normalized relative path that cannot leave its root (no `..`, no absolute, no `.git`). */
export function validRelativePath(value: unknown): value is string {
	if (typeof value !== 'string' || !value || value.length > MAX_PATH || /[\0\\]/.test(value) || path.isAbsolute(value)) return false;
	if (path.posix.normalize(value) !== value || value.endsWith('/')) return false;
	const segments = value.split('/');
	return !segments.some(segment => segment === '..' || segment === '.' || !segment) && segments[0] !== '.git';
}
export function validateWorktreeSettings(value: unknown): WorktreeSettings {
	if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('worktree must be an object');
	const record = value as Record<string, unknown>;
	for (const key of Object.keys(record)) if (!['location', 'symlink', 'files', 'branchFrom', 'branchName', 'hook'].includes(key)) throw new Error(`Unknown worktree setting: ${key}`);
	if (record.branchFrom !== undefined && !BRANCH_FROM.includes(record.branchFrom as BranchFrom)) throw new Error(`worktree.branchFrom must be one of ${BRANCH_FROM.join(', ')}`);
	if (record.hook !== undefined && typeof record.hook !== 'boolean') throw new Error('worktree.hook must be true or false');
	if (record.branchName !== undefined) {
		if (typeof record.branchName !== 'string' || !record.branchName.includes('{name}')) throw new Error('worktree.branchName must be a template containing {name}');
		let example: string;
		try { example = expandBranchName(record.branchName, {name: 'name', user: 'user'}); } catch (error) { throw new Error(`Invalid worktree.branchName: ${error instanceof Error ? error.message : String(error)}`); }
		const problem = branchNameProblem(example);
		if (problem) throw new Error(`Invalid worktree.branchName ${JSON.stringify(record.branchName)}: ${problem}`);
	}
	if (record.location !== undefined) {
		const location = templateString(record.location, 'worktree.location');
		if (!location.includes('{name}')) throw new Error('worktree.location must contain {name}');
		if (!path.isAbsolute(expandWorktreeTemplate(location, DUMMY))) throw new Error('worktree.location must be absolute (start with /, ~/, {home}, {repoParent} or {repoRoot})');
	}
	if (record.symlink !== undefined) {
		if (!Array.isArray(record.symlink)) throw new Error('worktree.symlink must be a list of relative paths');
		if (record.symlink.length > MAX_ENTRIES) throw new Error(`At most ${MAX_ENTRIES} worktree.symlink entries are supported`);
		for (const entry of record.symlink) if (!validRelativePath(entry)) throw new Error(`Invalid worktree.symlink path: ${JSON.stringify(entry)}`);
	}
	if (record.files !== undefined) {
		if (!record.files || typeof record.files !== 'object' || Array.isArray(record.files)) throw new Error('worktree.files must map destinations to sources');
		const entries = Object.entries(record.files);
		if (entries.length > MAX_ENTRIES) throw new Error(`At most ${MAX_ENTRIES} worktree.files entries are supported`);
		for (const [destination, source] of entries) {
			if (!validRelativePath(destination) || ['__proto__', 'constructor', 'prototype'].includes(destination)) throw new Error(`Invalid worktree.files destination: ${JSON.stringify(destination)}`);
			templateString(source, `worktree.files source for ${destination}`);
		}
	}
	return record as WorktreeSettings;
}
/**
 * Repository location/branchFrom/branchName/hook win; symlinks are a deduplicated union (global first);
 * files merge by destination (repository wins).
 */
export function mergeWorktreeSettings(global: WorktreeSettings | undefined, repository: WorktreeSettings | undefined): WorktreeSettings | undefined {
	if (!global && !repository) return undefined;
	const merged: WorktreeSettings = {};
	for (const key of ['location', 'branchFrom', 'branchName', 'hook'] as const) {
		const value = repository?.[key] ?? global?.[key];
		if (value !== undefined && value !== '') (merged as Record<string, unknown>)[key] = value;
	}
	if (global?.symlink || repository?.symlink) merged.symlink = [...new Set([...global?.symlink ?? [], ...repository?.symlink ?? []])];
	if (global?.files || repository?.files) merged.files = {...global?.files, ...repository?.files};
	return merged;
}

const inside = (root: string, target: string) => target === root || target.startsWith(`${root}${path.sep}`);
/**
 * Creates the parent directories of `relative` inside `root` one segment at a time, refusing to pass
 * through anything (a symlink or other non-directory) that resolves outside the worktree.
 */
async function ensureParent(root: string, relative: string): Promise<string> {
	let current = root;
	for (const segment of path.dirname(relative).split('/').filter(part => part && part !== '.')) {
		current = path.join(current, segment);
		const stat = await fs.lstat(current).catch(error => { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; });
		if (!stat) await fs.mkdir(current);
		else if (stat.isSymbolicLink()) { if (!inside(root, await fs.realpath(current))) throw new Error(`parent ${path.relative(root, current)} leaves the worktree`); }
		else if (!stat.isDirectory()) throw new Error(`parent ${path.relative(root, current)} is not a directory`);
	}
	const parent = await fs.realpath(current);
	if (!inside(root, parent)) throw new Error('parent leaves the worktree');
	return parent;
}
/** Links one destination; existing real content is never replaced, an existing symlink is replaced atomically. */
async function linkOne(root: string, destination: string, source: string): Promise<'linked' | string> {
	if (!validRelativePath(destination)) return 'invalid destination';
	let target: string;
	try { target = await fs.realpath(source); }
	catch (error) { return (error as NodeJS.ErrnoException).code === 'ENOENT' ? 'source missing, skipped' : `source unavailable: ${(error as Error).message}`; }
	const parent = await ensureParent(root, destination);
	const file = path.join(parent, path.basename(destination));
	const existing = await fs.lstat(file).catch(() => undefined);
	if (existing && !existing.isSymbolicLink()) return 'exists and is not a symlink, kept';
	if (inside(file, target)) return 'source is the destination itself, skipped';
	const temporary = path.join(parent, `.deckhand-link-${randomUUID().slice(0, 8)}`);
	await fs.symlink(target, temporary);
	try { await fs.rename(temporary, file); }
	catch (error) { await fs.unlink(temporary).catch(() => {}); throw error; }
	return 'linked';
}
/**
 * Applies `symlink` and `files` to a newly created worktree. Never throws: every skipped or failed entry
 * becomes a note, so linking never fails session creation.
 */
export async function applyWorktreeLinks(settings: WorktreeSettings | undefined, options: {launchRoot: string; worktreeRoot: string; vars: TemplateVars}): Promise<LinkResult> {
	const result: LinkResult = {linked: [], notes: []};
	if (!settings) return result;
	let root: string;
	try { root = await fs.realpath(options.worktreeRoot); }
	catch (error) { result.notes.push(`worktree unavailable: ${(error as Error).message}`); return result; }
	const entries: Array<[string, string]> = [
		...(settings.symlink ?? []).map(entry => [entry, path.join(options.launchRoot, entry)] as [string, string]),
		...Object.entries(settings.files ?? {}).map(([destination, source]) => {
			let resolved: string;
			try { resolved = expandWorktreeTemplate(source, options.vars); } catch { resolved = ''; }
			return [destination, resolved && path.resolve(options.launchRoot, resolved)] as [string, string];
		}),
	];
	for (const [destination, source] of entries) {
		try {
			const outcome = source ? await linkOne(root, destination, source) : 'invalid source';
			if (outcome === 'linked') result.linked.push(destination); else result.notes.push(`${destination}: ${outcome}`);
		} catch (error) { result.notes.push(`${destination}: ${error instanceof Error ? error.message : String(error)}`); }
	}
	return result;
}
