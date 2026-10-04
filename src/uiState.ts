import fs from 'node:fs/promises';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {getConfigDir} from './paths.js';
import type {RightPaneTab} from './types.js';
import {SESSION_FILTERS, type SessionFilter} from './sessionFeatures.js';
export interface UiState {
	selectedId?: string; activeTab?: RightPaneTab; sidebarWidth?: number;
	sessionFilter?: SessionFilter; sessionQuery?: string;
	sessionTabs: Record<string, RightPaneTab>; collapsedSessionIds: string[]; hiddenExitedSessionIds: string[];
}
const tabs = new Set(['preview', 'terminal', 'git', 'dev', 'notes']);
export function normalizeUiState(raw: unknown): UiState {
	const value = raw && typeof raw === 'object' ? raw as Record<string, unknown> : {};
	const strings = (input: unknown): string[] => Array.isArray(input) ? input.filter((id): id is string => typeof id === 'string').slice(0, 10000) : [];
	return {
		selectedId: typeof value.selectedId === 'string' ? value.selectedId : undefined,
		activeTab: typeof value.activeTab === 'string' && tabs.has(value.activeTab) ? value.activeTab as RightPaneTab : undefined,
		sessionFilter: typeof value.sessionFilter === 'string' && SESSION_FILTERS.includes(value.sessionFilter as SessionFilter) ? value.sessionFilter as SessionFilter : 'active',
		sessionQuery: typeof value.sessionQuery === 'string' ? value.sessionQuery.slice(0, 4096) : '',
		sidebarWidth: typeof value.sidebarWidth === 'number' && Number.isFinite(value.sidebarWidth) ? value.sidebarWidth : undefined,
		sessionTabs: Object.fromEntries(Object.entries(value.sessionTabs && typeof value.sessionTabs === 'object' ? value.sessionTabs : {}).filter(([, tab]) => typeof tab === 'string' && tabs.has(tab))) as Record<string, RightPaneTab>,
		collapsedSessionIds: strings(value.collapsedSessionIds), hiddenExitedSessionIds: strings(value.hiddenExitedSessionIds),
	};
}
async function readAll(): Promise<Record<string, UiState>> {
	try { const value = JSON.parse(await fs.readFile(path.join(getConfigDir(), 'ui-state.json'), 'utf8')); return value && typeof value === 'object' && !Array.isArray(value) ? value : {}; }
	catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT' || error instanceof SyntaxError) return {}; throw error; }
}
// UI preferences are a convenience: unreadable state falls back to defaults.
export async function loadUiState(repoRoot: string): Promise<UiState> {
	try { return normalizeUiState((await readAll())[path.resolve(repoRoot)]); }
	catch { return normalizeUiState(undefined); }
}
let queue = Promise.resolve();
export function saveUiState(repoRoot: string, state: UiState): Promise<void> {
	const snapshot = normalizeUiState(JSON.parse(JSON.stringify(state)));
	const operation = queue.then(async () => {
		await fs.mkdir(getConfigDir(), {recursive: true});
		const all = await readAll(); all[path.resolve(repoRoot)] = snapshot;
		const file = path.join(getConfigDir(), 'ui-state.json'); const temp = `${file}.${randomUUID()}.tmp`;
		await fs.writeFile(temp, `${JSON.stringify(all, null, 2)}\n`, {mode: 0o600}); await fs.rename(temp, file);
	});
	queue = operation.catch(() => {});
	return operation;
}
