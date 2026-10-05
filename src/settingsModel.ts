import {validateProjectConfig, type SaveTrust} from './projectConfig.js';
import type {ConfigTargetKind, ProjectConfigDocument} from './projectConfigDocument.js';
import type {SettingsInfo, WorktreeCandidate} from './settingsInfo.js';
import {worktreeLocation, type TemplateVars} from './worktreeLinks.js';

// The Settings screen (C) as pure data: a grid with one row per setting and one column per layer (global defaults,
// this repo's deckhand.json), each cell that layer's own value and whether it applies (from explainSettings); the
// options of each control; and the edits as JSON written into the selected column's document.

export type SettingId = 'defaultAgent' | 'defaultWorkspace' | 'devCommand' | 'setupCommand' | 'actions' | 'worktree.location' | 'worktree.branchFrom' | 'worktree.branchName' | 'worktree.links' | 'worktree.hook' | 'agentHooks' | 'notifications';
export type SettingControl = 'choice' | 'text' | 'actions' | 'links';
export interface SettingDef {id: SettingId; label: string; section: 'General' | 'Commands' | 'Worktrees' | 'Agents'; control: SettingControl; /** Stored in config.json itself (not a layer): about this machine, never set by a repository. */ globalOnly?: boolean}
export const SETTINGS: readonly SettingDef[] = [
	{id: 'defaultAgent', label: 'Default agent', section: 'General', control: 'choice'},
	{id: 'defaultWorkspace', label: 'Default workspace', section: 'General', control: 'choice'},
	{id: 'devCommand', label: 'Dev command', section: 'Commands', control: 'text'},
	{id: 'setupCommand', label: 'Setup command', section: 'Commands', control: 'text'},
	{id: 'actions', label: 'Actions', section: 'Commands', control: 'actions'},
	{id: 'worktree.location', label: 'Location', section: 'Worktrees', control: 'choice'},
	{id: 'worktree.branchFrom', label: 'Branch from', section: 'Worktrees', control: 'choice'},
	{id: 'worktree.branchName', label: 'Branch name', section: 'Worktrees', control: 'text'},
	{id: 'worktree.links', label: 'Linked items', section: 'Worktrees', control: 'links'},
	{id: 'worktree.hook', label: 'Creation hook', section: 'Worktrees', control: 'choice'},
	{id: 'agentHooks', label: 'Agent signals', section: 'Agents', control: 'choice', globalOnly: true},
	{id: 'notifications', label: 'Notifications', section: 'Agents', control: 'choice', globalOnly: true},
];
/** The config.json key of a global-only setting. */
export const APP_FLAG = {agentHooks: 'agent_hooks', notifications: 'notifications'} as const;
export type AppFlagId = keyof typeof APP_FLAG;
export const isAppFlag = (id: SettingId): id is AppFlagId => Object.hasOwn(APP_FLAG, id);
const appFlag = (info: Pick<SettingsInfo, 'agentHooks' | 'notifications'>, id: AppFlagId) => info[id] === true;
const LOCATION_PRESETS = {next: '{repoParent}/worktrees/{name}', inside: '{repoRoot}/.worktrees/{name}'} as const;
const WORKSPACE_LABELS: Record<string, string> = {none: 'no worktree', new: 'new worktree', existing: 'existing worktree'};

/** The JSON path a setting is stored at (Linked items edits worktree.symlink; worktree.files stays raw-JSON only). */
export function settingPath(id: SettingId, entry?: string): string[] {
	if (id === 'actions') return entry === undefined ? ['actions'] : ['actions', entry];
	if (id === 'worktree.links') return ['worktree', 'symlink'];
	return id.split('.');
}

export type Layer = Record<string, unknown>;
const layerLabel = (kind: ConfigTargetKind) => kind === 'global' ? 'Global defaults are' : 'deckhand.json is';
/** A document's settings as written (trusted or not); a file that does not exist yet is empty. */
function documentLayer(document: Pick<ProjectConfigDocument, 'raw' | 'exists' | 'kind'>): Layer {
	let value: unknown;
	try { value = document.exists ? JSON.parse(document.raw.replace(/^﻿/, '')) : {}; }
	catch { throw new Error(`${layerLabel(document.kind)} not valid JSON; press e to repair it first`); }
	if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${layerLabel(document.kind)} not a JSON object; press e to repair it first`);
	return value as Layer;
}
export function infoLayer(info: Pick<SettingsInfo, 'targets'>, kind: ConfigTargetKind): Layer | undefined {
	const document = info.targets[kind];
	if (!document) return undefined;
	try { return documentLayer(document); } catch { return undefined; }
}
function valueAt(layer: Layer | undefined, keys: string[]): unknown {
	let current: unknown = layer;
	for (const key of keys) {
		if (!current || typeof current !== 'object' || Array.isArray(current) || !Object.hasOwn(current, key)) return undefined;
		current = (current as Layer)[key];
	}
	return current;
}
/** Whether `layer` (as written) sets the setting; Linked items count the symlink list and files map. */
export function layerSets(layer: Layer | undefined, id: SettingId, entry?: string): boolean {
	if (id === 'worktree.links') return valueAt(layer, ['worktree', 'symlink']) !== undefined || valueAt(layer, ['worktree', 'files']) !== undefined;
	return valueAt(layer, settingPath(id, entry)) !== undefined;
}
export const otherTarget = (kind: ConfigTargetKind): ConfigTargetKind => kind === 'global' ? 'repository' : 'global';
export function targetName(kind: ConfigTargetKind): string { return kind === 'global' ? 'global defaults' : "this repo's deckhand.json"; }

/** A change to one setting in one layer: `value` undefined removes it. */
export interface SettingChange {path: string[]; value?: unknown}
/**
 * Pure: `document`'s JSON with one key set or removed (empty `actions`/`worktree` objects left behind are removed);
 * every other key keeps its value and place. The result is validated like a save.
 */
export function applyChange(document: Pick<ProjectConfigDocument, 'raw' | 'exists' | 'kind'>, change: SettingChange): string {
	const next = structuredClone(documentLayer(document));
	const parents: Layer[] = [next];
	for (const key of change.path.slice(0, -1)) {
		const parent = parents.at(-1)!;
		const child = parent[key];
		if (child === undefined) { if (change.value === undefined) break; parent[key] = {}; }
		else if (!child || typeof child !== 'object' || Array.isArray(child)) throw new Error(`${change.path.slice(0, parents.length).join('.')} is not an object; press e to repair it first`);
		parents.push(parent[key] as Layer);
	}
	const last = change.path.at(-1)!;
	if (parents.length === change.path.length) {
		if (change.value === undefined) delete parents.at(-1)![last]; else parents.at(-1)![last] = change.value;
	}
	for (let depth = parents.length - 1; depth > 0; depth--) if (!Object.keys(parents[depth]!).length) delete parents[depth - 1]![change.path[depth - 1]!];
	validateProjectConfig(next, document.kind === 'global' ? 'defaults' : undefined);
	return `${JSON.stringify(next, null, 2)}\n`;
}
/** The value `layer` stores for a setting (undefined when it does not set it). */
export function ownValue(layer: Layer | undefined, id: SettingId, entry?: string): unknown { return valueAt(layer, settingPath(id, entry)); }

// Grid ----------------------------------------------------------------------------------------------------------

/** The grid's value columns, left to right: each shows that layer's own stored value. */
export const COLUMNS: readonly ConfigTargetKind[] = ['global', 'repository'];
/** Column headings, longest first (narrow panes take the shorter). */
export const COLUMN_TITLES: Record<ConfigTargetKind, string[]> = {global: ['Global (all repos)', 'Global'], repository: ['This repo']};
export const layerName = (kind: ConfigTargetKind) => kind === 'global' ? 'Global' : 'This repo';
/** The column Settings opens on: this repo when there is a repository layer, else global. */
export function initialColumn(info: Pick<SettingsInfo, 'targets'>): ConfigTargetKind { return info.targets.repository ? 'repository' : 'global'; }
/** Why `column`'s layer cannot be edited here (no repository, unreadable global defaults), or undefined. */
export function columnProblem(info: Pick<SettingsInfo, 'targets'>, column: ConfigTargetKind): string | undefined {
	if (column === 'repository' && !info.targets.repository) return `No repository layer here: ${info.targets.repositoryError ?? 'no deckhand.json'}`;
	if (column === 'global' && !info.targets.global) return info.targets.globalError ?? 'Global defaults are unavailable';
	return undefined;
}

/** One cell: a layer's own value for one setting, and whether it is what applies now. */
export interface GridCell {
	/** The layer's value for display ('—' when unset). The global column shows the built-in value when that applies. */
	text: string;
	/** The full value for the details line (a list's entries, an expanded path). */
	full?: string;
	/** A template as written, when `text` is its expansion. */
	template?: string;
	/** The layer's file stores a value here. */
	set: boolean;
	/** This value is (part of) what applies now (explainSettings ≡ resolveSettings). */
	effective: boolean;
	/** The built-in default: neither layer's value applies. */
	builtIn?: boolean;
	/** The legacy dev_command (config.json, outside `defaults`). */
	legacy?: boolean;
	/** A repository value that applies only once deckhand.json is trusted. */
	needsTrust?: boolean;
	/** A global-only setting's This repo cell. */
	globalOnly?: boolean;
	/** A short warning shown beside the value (e.g. Codex hooks not set up); the row note explains it. */
	warning?: string;
}
export interface GridRow {def: SettingDef; cells: Record<ConfigTargetKind, GridCell>; /** Why the value is ignored, or the hook file's state. */ note?: string; /** The note is a warning (shown in yellow). */ warn?: boolean}
export function homePath(value: string, home?: string): string {
	return home && (value === home || value.startsWith(`${home}/`)) ? `~${value.slice(home.length)}` : value;
}
function branchFromLabel(value: string, info: Pick<SettingsInfo, 'defaultBranch' | 'originBranch'>): string {
	if (value === 'default') return `default branch (${info.defaultBranch ?? 'none found'})`;
	if (value === 'origin') return info.originBranch ? `origin/${info.originBranch}, fetched` : 'origin (no remote found)';
	return 'current checkout';
}
const BUILT_IN_VALUE: Partial<Record<SettingId, string | boolean>> = {defaultAgent: 'claude', defaultWorkspace: 'none', devCommand: 'dev', 'worktree.branchFrom': 'current', 'worktree.branchName': '{name}', 'worktree.hook': true};
type GridInfo = Pick<SettingsInfo, 'rows' | 'targets' | 'vars' | 'defaultBranch' | 'originBranch' | 'defaultLocation' | 'hookFile' | 'agentHooks' | 'notifications' | 'codexHooks'>;
/** The hint under Agent signals when Codex's own hook config doesn't call this Deckhand (one line, so it stays short). */
export function codexHookNote(status: NonNullable<SettingsInfo['codexHooks']>, home?: string): string {
	const file = homePath(status.file, home);
	if (status.state === 'other') return `Codex hooks call another Deckhand install: redo ${status.command}, then /hooks in Codex`;
	return `Codex hooks not set up: ${status.command} ${status.fileExists ? `→ merge into ${file}` : `> ${file}`}, then /hooks in Codex`;
}

/**
 * The Settings grid in screen order: per setting, each layer's own value as written (trusted or not) and which one
 * applies now. What applies comes from explainSettings, so it equals resolveSettings: an untrusted repository value
 * is shown in its cell (needsTrust) while the global or built-in value keeps the emphasis; defaultAgent and
 * defaultWorkspace (and hook: false) apply untrusted too.
 */
export function settingsGrid(info: GridInfo): GridRow[] {
	const home = info.vars?.home;
	const layers: Record<ConfigTargetKind, Layer | undefined> = {global: infoLayer(info, 'global'), repository: infoLayer(info, 'repository')};
	const sourceOf: Record<ConfigTargetKind, string> = {global: 'global', repository: 'repo'};
	const show = (id: SettingId, value: unknown): Pick<GridCell, 'text' | 'template' | 'full'> => {
		if (id === 'defaultWorkspace') return {text: WORKSPACE_LABELS[String(value)] ?? String(value)};
		if (id === 'worktree.branchFrom') return {text: branchFromLabel(String(value), info)};
		if (id === 'worktree.hook') return {text: value === false ? 'off' : 'on'};
		if (id === 'worktree.location') { const text = previewLocation(String(value), info.vars); return {text, full: text, template: String(value)}; }
		return {text: String(value), full: String(value)};
	};
	const hookRow = info.rows.find(row => row.key === 'worktree.hook');
	return SETTINGS.map(def => {
		if (isAppFlag(def.id)) {
			const on = appFlag(info, def.id);
			const codex = def.id === 'agentHooks' && on ? info.codexHooks : undefined;
			const global: GridCell = {text: on ? 'on' : 'off', set: on, effective: true, ...on ? {} : {builtIn: true}, ...codex ? {warning: 'Codex'} : {}};
			return {def, cells: {global, repository: {text: 'global only', set: false, effective: false, globalOnly: true}}, ...codex ? {note: codexHookNote(codex, home), warn: true} : {}};
		}
		if (def.control === 'actions' || def.control === 'links') {
			const keys = def.control === 'actions' ? ['actions'] : ['worktree.symlink', 'worktree.files'];
			const rows = info.rows.filter(row => keys.includes(row.key) && row.entry !== undefined);
			const cell = (kind: ConfigTargetKind): GridCell => {
				const own = def.control === 'actions' ? Object.keys(record(ownValue(layers[kind], 'actions'))) : [...strings(valueAt(layers[kind], ['worktree', 'symlink'])), ...Object.keys(record(valueAt(layers[kind], ['worktree', 'files'])))];
				const text = !own.length ? '—' : def.control === 'actions' ? own.join(', ') : `${own.length} item${own.length === 1 ? '' : 's'}`;
				return {text, full: own.join(', '), set: own.length > 0, effective: rows.some(row => row.source === sourceOf[kind]), ...kind === 'repository' && rows.some(row => row.pending) ? {needsTrust: true} : {}};
			};
			return {def, cells: {global: cell('global'), repository: cell('repository')}};
		}
		const row = info.rows.find(entry => entry.key === def.id);
		const source = row?.source ?? 'not set';
		const cell = (kind: ConfigTargetKind): GridCell => {
			const own = ownValue(layers[kind], def.id);
			const effective = source === sourceOf[kind];
			if (own !== undefined) return {...show(def.id, own), set: true, effective, ...kind === 'repository' && !effective && row?.pending ? {needsTrust: true} : {}};
			if (kind === 'global' && source === 'legacy dev_command' && typeof row?.raw === 'string') return {...show(def.id, row.raw), set: false, effective: true, legacy: true};
			if (kind === 'global' && source === 'built-in default') {
				const value = def.id === 'worktree.location' ? homePath(info.defaultLocation, home) : BUILT_IN_VALUE[def.id];
				if (value !== undefined) return {...def.id === 'worktree.location' ? {text: String(value), full: String(value)} : show(def.id, value), set: false, effective: true, builtIn: true};
			}
			return {text: '—', set: false, effective: false};
		};
		const ignored = def.id.startsWith('worktree.') && def.id !== 'worktree.hook' && row?.note?.startsWith('ignored') ? 'ignored while the creation hook is active' : undefined;
		const note = def.id === 'worktree.hook' ? (info.hookFile ? hookRow?.note : 'no .claude/scripts/create-worktree.sh in this repo') : ignored;
		return {def, cells: {global: cell('global'), repository: cell('repository')}, ...note ? {note} : {}};
	});
}
const record = (value: unknown): Record<string, unknown> => value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
const strings = (value: unknown): string[] => Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];

/** What a built-in value (or a setting nothing sets) means in practice: one line for the details box. */
const BUILT_IN: Record<SettingId, string> = {
	defaultAgent: 'Nothing sets it, so new sessions (n) start on claude.',
	defaultWorkspace: 'Nothing sets it, so n offers "no worktree" (this checkout).',
	devCommand: 'No Dev command is set, so d runs a shell command named `dev`.',
	setupCommand: 'No setup command: new worktrees start the agent right away.',
	actions: 'No actions yet: named commands you run with e on a session.',
	'worktree.location': "Nothing sets it: new worktrees go in Deckhand's own folder.",
	'worktree.branchFrom': 'Nothing sets it: new branches start at the current checkout.',
	'worktree.branchName': 'Nothing sets it: new branches are named after the session.',
	'worktree.links': 'Nothing linked: new worktrees start without untracked files.',
	'worktree.hook': 'On by default: a trusted create-worktree.sh makes worktrees.',
	agentHooks: 'Off: attention (! and the session markers) comes from terminal activity only.',
	notifications: 'Off: no desktop notifications.',
};
/** What a global-only setting does when on (Notifications also says what Agent signals adds). */
function appFlagDetail(info: Pick<SettingsInfo, 'agentHooks' | 'notifications'>, id: AppFlagId): string {
	if (!appFlag(info, id)) return BUILT_IN[id];
	if (id === 'agentHooks') return 'new sessions report working / needs input / done · Claude automatically, Codex via its own hooks';
	return info.agentHooks ? 'a desktop notification when a session needs you or exits' : 'a desktop notification when a session exits (Agent signals adds needs input / done)';
}
export const NEEDS_TRUST_DETAIL = "Applies once trusted — you'll be asked the first time it runs (or press T)";
/** The details line of the selected cell: its layer (or "Built-in default") and how it relates to the other layer. */
export interface CellDetail {head: string; layer?: ConfigTargetKind; relation: string; warn?: boolean; error?: boolean}
export function cellDetail(info: Pick<SettingsInfo, 'targets' | 'repository' | 'agentHooks' | 'notifications'>, row: GridRow, column: ConfigTargetKind): CellDetail {
	if (isAppFlag(row.def.id)) {
		if (column === 'repository') return {head: 'This repo', layer: column, relation: 'global only: it is about your machine, not the repo'};
		return {head: 'Global', layer: column, relation: appFlagDetail(info, row.def.id)};
	}
	const problem = columnProblem(info, column);
	if (problem) return {head: layerName(column), layer: column, relation: problem, error: true};
	if (column === 'repository' && info.repository.state === 'invalid') return {head: layerName(column), layer: column, relation: `invalid (${info.repository.error ?? 'unknown error'}): e repairs it`, error: true};
	const cell = row.cells[column], other = row.cells[otherTarget(column)];
	const list = row.def.control === 'actions' || row.def.control === 'links';
	const base = {head: layerName(column), layer: column};
	if (cell.builtIn) return {head: 'Built-in default', relation: BUILT_IN[row.def.id]};
	if (cell.legacy) return {...base, relation: 'the legacy dev_command in config.json; setting a global Dev command replaces it'};
	if (cell.needsTrust) return {...base, relation: NEEDS_TRUST_DETAIL, warn: true};
	if (!cell.set) {
		if (column === 'global') return {...base, relation: other.effective ? 'not set; this repo sets it' : BUILT_IN[row.def.id]};
		return {...base, relation: other.builtIn ? 'not set: the built-in default applies' : other.effective ? 'not set: inherits global' : 'not set'};
	}
	if (!cell.effective) return {...base, relation: column === 'global' ? 'overridden by this repo' : 'not in effect'};
	if (column === 'repository') {
		const relation = list && other.effective ? 'merged with global (same names: this repo wins)' : other.set || other.legacy ? 'overrides global' : 'applies here';
		// What an untrusted file may still do: preselect the n picker, or switch the creation hook off.
		const untrusted = info.repository.state !== 'trusted' ? row.def.id === 'worktree.hook' ? 'it can only switch the hook off' : row.def.section === 'General' ? 'it only preselects n' : undefined : undefined;
		return {...base, relation: untrusted ? `${relation}, untrusted too: ${untrusted}` : relation};
	}
	return {...base, relation: list && other.effective ? 'merged with this repo' : other.needsTrust ? 'applies until this repo\'s value is trusted' : 'inherited by this repo'};
}

/** Settings → Actions for one layer: its own actions (editable), then the other layer's as dimmed context. */
export interface ActionEntry {name: string; command: string; status?: string; needsTrust?: boolean}
export function layerActions(info: Pick<SettingsInfo, 'rows' | 'targets'>, kind: ConfigTargetKind): {own: ActionEntry[]; context: ActionEntry[]} {
	const commands = (layer: ConfigTargetKind) => Object.entries(record(ownValue(infoLayer(info, layer), 'actions'))).filter((entry): entry is [string, string] => typeof entry[1] === 'string');
	const pending = new Set(info.rows.filter(row => row.key === 'actions' && row.pending && row.entry !== undefined).map(row => row.entry!));
	const mine = commands(kind), theirs = commands(otherTarget(kind));
	const names = (list: [string, string][]) => new Set(list.map(([name]) => name));
	const own = mine.map(([name, command]): ActionEntry => {
		if (kind === 'repository' && pending.has(name)) return {name, command, status: '⚠ needs trust', needsTrust: true};
		return {name, command, ...names(theirs).has(name) ? {status: kind === 'repository' ? 'overrides global' : 'this repo overrides it'} : {}};
	});
	const context = theirs.filter(([name]) => !names(mine).has(name)).map(([name, command]): ActionEntry => ({name, command, status: kind === 'repository' ? 'global' : pending.has(name) ? 'this repo · ⚠ needs trust' : 'this repo'}));
	return {own, context};
}

// Controls ------------------------------------------------------------------------------------------------------

export interface ChoiceOption {label: string; value?: string | boolean; detail?: string; /** detail is a path (cut from the front). */ path?: boolean; /** Shown on its own line under the option, never after the detail. */ warning?: string; /** Opens a text input (custom location). */ custom?: boolean}
export function previewLocation(template: string, vars: TemplateVars | undefined): string {
	if (!vars) return template;
	try { return homePath(worktreeLocation(template, vars), vars.home); } catch (error) { return `invalid: ${error instanceof Error ? error.message : String(error)}`; }
}
/** The options of a choice setting for `target`; the location's first option (no value) removes the target's location. */
export function choiceOptions(info: SettingsInfo, id: SettingId, target: ConfigTargetKind): ChoiceOption[] {
	switch (id) {
		case 'defaultAgent': return ['claude', 'pi', 'codex'].map(value => ({label: value, value}));
		case 'defaultWorkspace': return ['none', 'new', 'existing'].map(value => ({label: WORKSPACE_LABELS[value]!, value}));
		case 'worktree.branchFrom': return ['current', 'default', 'origin'].map(value => ({label: branchFromLabel(value, info), value}));
		case 'agentHooks': return [
			{label: 'on', value: true, detail: 'Claude: automatic · Codex: needs its hooks set up'},
			{label: 'off', value: false, detail: 'attention from terminal activity only'},
		];
		case 'notifications': return [
			{label: 'on', value: true, detail: 'desktop notification when a session needs you or exits'},
			{label: 'off', value: false},
		];
		case 'worktree.hook': return [
			{label: 'on', value: true, detail: info.hookFile ? 'create-worktree.sh runs once trusted' : 'no create-worktree.sh found'},
			{label: 'off', value: false, detail: 'Deckhand creates worktrees itself'},
		];
		case 'worktree.location': {
			const global = target === 'repository' ? ownValue(infoLayer(info, 'global'), 'worktree.location') : undefined;
			const inherited = typeof global === 'string' ? {label: 'global default', detail: previewLocation(global, info.vars), path: true} : {label: 'Deckhand default', detail: homePath(info.defaultLocation, info.vars?.home), path: true};
			const own = ownValue(infoLayer(info, target), id);
			const custom = typeof own === 'string' && own !== LOCATION_PRESETS.next && own !== LOCATION_PRESETS.inside ? own : undefined;
			return [
				inherited,
				{label: 'next to repo', value: LOCATION_PRESETS.next, detail: previewLocation(LOCATION_PRESETS.next, info.vars), path: true},
				{label: 'inside repo', value: LOCATION_PRESETS.inside, detail: previewLocation(LOCATION_PRESETS.inside, info.vars), path: true, ...info.insideIgnored ? {} : {warning: '⚠ .worktrees/ is not gitignored — worktrees would show up as untracked files'}},
				{label: 'custom…', custom: true, ...custom ? {value: custom, detail: custom, path: true} : {detail: 'type a template with {name}'}},
			];
		}
		default: return [];
	}
}
/**
 * The option to highlight first (`index`) and the one this layer stores (`current`, marked ◉; -1 when it stores none):
 * the layer's own value, else the value in effect. The location's first option (inherit) is current when unset.
 */
export function currentChoice(info: SettingsInfo, id: SettingId, target: ConfigTargetKind, options: ChoiceOption[]): {index: number; current: number} {
	if (isAppFlag(id)) { const index = options.findIndex(option => option.value === appFlag(info, id)); return {index, current: appFlag(info, id) ? index : -1}; }
	const own = ownValue(infoLayer(info, target), id);
	if (id === 'worktree.location') {
		if (own === undefined) return {index: 0, current: 0};
		const found = options.findIndex(option => !option.custom && option.value === own);
		const index = found >= 0 ? found : options.length - 1;
		return {index, current: index};
	}
	if (own !== undefined) { const index = options.findIndex(option => option.value === own); return {index: Math.max(0, index), current: index}; }
	const row = info.rows.find(entry => entry.key === id);
	const value = row?.raw ?? (id === 'worktree.hook' ? row?.value !== 'off' : row?.value);
	return {index: Math.max(0, options.findIndex(option => option.value === value)), current: -1};
}
/** Choosing an option: the change to save, or undefined when the target already has exactly that value. */
export function choiceChange(info: SettingsInfo, id: SettingId, target: ConfigTargetKind, option: ChoiceOption): SettingChange | undefined {
	if (isAppFlag(id)) return appFlag(info, id) === option.value ? undefined : {path: [APP_FLAG[id]], value: option.value};
	const own = ownValue(infoLayer(info, target), id);
	return own === option.value ? undefined : {path: settingPath(id), value: option.value};
}

/** The text a text control starts from: the layer's own value; empty when it stores none (see inheritedHint). */
export function initialText(info: Pick<SettingsInfo, 'targets'>, id: SettingId, target: ConfigTargetKind, entry?: string): string {
	const own = ownValue(infoLayer(info, target), id, entry);
	return typeof own === 'string' ? own : '';
}
/** Shown in an empty text input: what applies when this layer stores nothing (the other layer's or the built-in value). */
export function inheritedHint(info: Pick<SettingsInfo, 'targets' | 'rows'>, id: SettingId, target: ConfigTargetKind, entry?: string): string | undefined {
	if (target === 'repository') {
		const global = ownValue(infoLayer(info, 'global'), id, entry);
		if (typeof global === 'string') return `global: ${global}`;
		const legacy = id === 'devCommand' ? info.rows.find(row => row.key === 'devCommand' && row.source === 'legacy dev_command') : undefined;
		if (typeof legacy?.raw === 'string') return `global: ${legacy.raw} (legacy)`;
	}
	const builtIn = id === 'devCommand' ? 'dev' : id === 'worktree.branchName' ? '{name}' : undefined;
	return builtIn ? `built-in: ${builtIn}` : undefined;
}
// Actions: what they are and what a name and a command accept, in plain words (the rules are validateProjectConfig's).
export const MAX_ACTIONS = 30;
export const MAX_ACTION_NAME = 48;
export const ACTIONS_INTRO = "Named shell commands you run with e on a session; they run in that session's worktree and show in the Dev pane.";
export const ACTION_NAME_RULES = 'letters, numbers, spaces, _ . - · starts with a letter or number · up to 48 characters';
export const ACTION_NAME_EXAMPLES = 'e.g. test · lint frontend · db.migrate';
export const ACTION_COMMAND_HELP = "Runs with your shell in the session's worktree, like typing it in a terminal · && pipes cd env vars all work";
export const ACTION_COMMAND_EXAMPLES = 'e.g. npm test · cd backend && .venv/bin/pytest -x · make lint';

/** Why `name` cannot name an action, in plain words, or undefined. A trailing space (a different name) is refused too. */
export function actionNameProblem(name: string): string | undefined {
	if (!name) return 'type a name first';
	if (name.startsWith(' ')) return "can't start with a space";
	if (!/^[a-zA-Z0-9]/.test(name)) return 'must start with a letter or number';
	const bad = Array.from(name).find(char => !/^[a-zA-Z0-9 _.-]$/.test(char));
	if (bad) return `can't contain ${bad === '\t' ? 'a tab' : /^[\p{L}\p{N}\p{P}\p{S}]$/u.test(bad) ? `"${bad}"` : `U+${bad.codePointAt(0)!.toString(16).toUpperCase().padStart(4, '0')}`}`;
	if (name.endsWith(' ')) return "can't end with a space";
	if (name.length > MAX_ACTION_NAME) return `too long: ${name.length} characters (up to ${MAX_ACTION_NAME})`;
	try { validateProjectConfig({actions: {[name]: 'x'}}); return undefined; }
	catch { return `${name} is a reserved name`; }
}
const layerShort = (kind: ConfigTargetKind) => kind === 'global' ? 'global defaults' : 'this repo';
/** How many actions `kind`'s document defines as written (MAX_ACTIONS is the most one layer may have). */
export function actionCount(info: Pick<SettingsInfo, 'targets'>, kind: ConfigTargetKind): number {
	const actions = ownValue(infoLayer(info, kind), 'actions');
	return actions && typeof actions === 'object' && !Array.isArray(actions) ? Object.keys(actions).length : 0;
}
/**
 * The live check of a new action's name for `target`: an error (invalid, or the layer is full) or what saving it will
 * do (add, replace the layer's own action, override or sit under the other layer's). Nothing typed yet: neither.
 */
export function actionNameCheck(info: Pick<SettingsInfo, 'targets'>, target: ConfigTargetKind, name: string): {error?: string; note?: string} {
	const other = otherTarget(target);
	const full = actionCount(info, target) >= MAX_ACTIONS ? `${layerShort(target)} already has ${MAX_ACTIONS} actions, the most allowed: remove one (x) first` : undefined;
	if (!name) return full ? {error: full} : {};
	const problem = actionNameProblem(name);
	if (problem) return {error: problem};
	if (layerSets(infoLayer(info, target), 'actions', name)) return {note: `replaces the existing ${name} action (${layerShort(target)})`};
	if (full) return {error: full};
	if (layerSets(infoLayer(info, other), 'actions', name)) return {note: target === 'repository' ? `overrides the global ${name} action in this repo` : `adds a global ${name} action; this repo's ${name} action still wins here`};
	return {note: `adds a new action to ${layerShort(target)}`};
}
/** Why `text` cannot be a command, in plain words, or undefined. */
export function commandProblem(text: string): string | undefined {
	if (!text.trim()) return 'type a command';
	if (text.includes('\0')) return "can't contain a NUL character";
	if (text.length > 8192) return `too long: ${text.length} characters (up to 8192)`;
	return undefined;
}
/** A typed value as a change, or the validation error to show while the input stays open. */
export function textChange(id: SettingId, text: string, entry?: string): {change: SettingChange} | {error: string} {
	const path = settingPath(id, entry);
	if (id === 'actions' && !text.trim()) return {error: 'Type a command for the action'};
	if ((id === 'devCommand' || id === 'setupCommand') && !text.trim()) return {error: 'Type a command (x clears the setting instead)'};
	const probe: Layer = {};
	let parent = probe;
	for (const key of path.slice(0, -1)) parent = (parent[key] = {}) as Layer;
	parent[path.at(-1)!] = text;
	try { validateProjectConfig(probe); } catch (error) { return {error: error instanceof Error ? error.message : String(error)}; }
	return {change: {path, value: text}};
}

/** Linked items: candidates start linked when configured, otherwise as suggested. */
export function initialLinks(candidates: WorktreeCandidate[]): Record<string, boolean> {
	return Object.fromEntries(candidates.map(candidate => [candidate.path, candidate.configured ? true : candidate.suggestion === 'link']));
}
export interface LinkSelection {symlink: string[]; added: string[]; removed: string[]; /** Unlinked here but still linked by the other layer. */ stillLinked: string[]}
/**
 * Pure: the target layer's new `worktree.symlink` for the picker's selection. Its own entries stay in order unless
 * unlinked, newly linked candidates are appended unless the other layer already links them, files rows are left
 * alone (raw JSON), and entries the picker does not know are kept.
 */
export function linkSelection(target: Layer | undefined, other: Layer | undefined, candidates: WorktreeCandidate[], links: Record<string, boolean>): LinkSelection {
	const list = (layer: Layer | undefined) => { const value = ownValue(layer, 'worktree.links'); return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []; };
	const own = list(target), others = new Set(list(other));
	const known = new Map(candidates.filter(candidate => candidate.configured !== 'files').map(candidate => [candidate.path, Boolean(links[candidate.path])]));
	const kept = own.filter(entry => known.get(entry) !== false);
	const added = [...known].filter(([entry, linked]) => linked && !own.includes(entry) && !others.has(entry)).map(([entry]) => entry);
	return {symlink: [...kept, ...added], added, removed: own.filter(entry => known.get(entry) === false), stillLinked: [...others].filter(entry => known.get(entry) === false)};
}

/** What a save through Deckhand did to the repository file's trust (see savedProjectTrust). */
export function trustNote(trust: SaveTrust | undefined): string {
	switch (trust) {
		case 'kept': return 'still trusted';
		case 'created': return 'created and trusted';
		case 'unreviewed': return "review required (the file had changes you haven't reviewed)";
		case 'hook': return "review required (the creation hook hasn't been reviewed)";
		default: return 'reviewed before anything from it runs';
	}
}
export function savedMessage(label: string, target: ConfigTargetKind, {cleared = false, trust}: {cleared?: boolean; trust?: SaveTrust} = {}): string {
	return `${cleared ? 'Cleared' : 'Saved'} ${label} ${cleared ? 'in' : 'to'} ${layerShort(target)}${target === 'repository' ? ` · ${trustNote(trust)}` : ''}`;
}
export function actionSavedMessage(name: string, target: ConfigTargetKind, trust?: SaveTrust): string {
	return `Saved action ${name} to ${layerShort(target)}${target === 'repository' ? ` · ${trustNote(trust)}` : ''} · run it with e on a session`;
}
