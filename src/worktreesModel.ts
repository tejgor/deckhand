import path from 'node:path';
import {formatAge} from './sidebarModel.js';
import type {SessionRecord, WorktreeOverview, WorktreeOverviewEntry} from './types.js';

// W, the worktree manager (src/worktreesFlow.tsx renders it): the repository's worktrees grouped by what to do with
// them. Pure, so the grouping and labels are unit-tested (tests/worktreesModel.test.ts).

/**
 * `ready`: merged (or nothing beyond the default branch) and deleting it with its branch loses nothing; `leftovers`:
 * merged, but it has uncommitted files or commits of its own still; `progress`: not merged; `idle`: not merged, nothing
 * running and untouched for IDLE_DAYS; `missing`: its directory is gone; `main`: the main checkout (never deleted).
 */
export type WorktreeGroup = 'ready' | 'leftovers' | 'progress' | 'idle' | 'missing' | 'main';
export const WORKTREE_GROUPS: WorktreeGroup[] = ['ready', 'leftovers', 'progress', 'idle', 'missing', 'main'];
export const GROUP_TITLES: Record<WorktreeGroup, string> = {
	ready: 'Merged · safe to delete',
	leftovers: 'Merged · has leftovers',
	progress: 'In progress',
	idle: 'Idle',
	missing: 'Missing',
	main: 'Main checkout',
};
export const IDLE_DAYS = 14;
const DAY = 24 * 60 * 60_000;

export type WorktreeRow =
	| {kind: 'heading'; group: WorktreeGroup; count: number}
	| {kind: 'worktree'; group: WorktreeGroup; entry: WorktreeOverviewEntry}
	| {kind: 'empty'; text: string};

/** The worktree's name in the list: its branch, else its directory. */
export function worktreeName(entry: WorktreeOverviewEntry): string {
	return entry.branch || `${path.basename(entry.path)} (detached)`;
}

/** How it got merged, or why it counts as merged (clean, and its commits are all in the default branch); undefined when not. */
export function mergedText(entry: WorktreeOverviewEntry, defaultBranch?: string): string | undefined {
	const markers = entry.markers;
	if (markers?.mergedAt) {
		if (markers.mergeDetected === 'pr') return 'PR merged';
		if (markers.mergeDetected === 'ancestor') return `merged into ${markers.mergeTargetBranch ?? defaultBranch ?? 'the default branch'}`;
		if (markers.mergeMarkedManually) return 'marked merged (M)';
		return `${markers.mergeMode === 'squash' ? 'squash-merged' : 'merged'}${markers.mergeTargetBranch ? ` into ${markers.mergeTargetBranch}` : ''}`;
	}
	// A branch with no commits of its own is done only once nothing else is left in it either (a fresh branch with
	// uncommitted work is work in progress).
	if (entry.aheadOfDefault === 0 && entry.head && entry.inspection?.safe) return `nothing beyond ${defaultBranch ?? 'the default branch'}`;
	return undefined;
}

/** The latest sign of life: its last commit, its record's creation, or a session's last update in it. */
export function lastActivity(entry: WorktreeOverviewEntry, sessions: SessionRecord[]): number | undefined {
	const times = [entry.lastCommitAt, entry.createdAt, ...sessions.filter(session => entry.sessionIds.includes(session.id)).map(session => session.updatedAt)]
		.map(value => (value ? Date.parse(value) : NaN)).filter(value => Number.isFinite(value));
	return times.length ? Math.max(...times) : undefined;
}

export function worktreeGroup(entry: WorktreeOverviewEntry, sessions: SessionRecord[], now: number, defaultBranch?: string): WorktreeGroup {
	if (entry.isMain) return 'main';
	if (entry.missing) return 'missing';
	if (mergedText(entry, defaultBranch)) return entry.inspection?.safe ? 'ready' : 'leftovers';
	const active = lastActivity(entry, sessions);
	if (!entry.runningIds.length && active !== undefined && now - active > IDLE_DAYS * DAY) return 'idle';
	return 'progress';
}

/** The list: each group that has worktrees under its heading (most recently active first), the main checkout last. */
export function worktreeRows(overview: WorktreeOverview | undefined, sessions: SessionRecord[], now: number): WorktreeRow[] {
	if (!overview) return [];
	const groups = new Map<WorktreeGroup, WorktreeOverviewEntry[]>();
	for (const entry of overview.entries) {
		const group = worktreeGroup(entry, sessions, now, overview.defaultBranch);
		groups.set(group, [...groups.get(group) ?? [], entry]);
	}
	const rows: WorktreeRow[] = [];
	for (const group of WORKTREE_GROUPS) {
		const entries = groups.get(group);
		if (!entries?.length) continue;
		const sorted = [...entries].sort((left, right) => (lastActivity(right, sessions) ?? 0) - (lastActivity(left, sessions) ?? 0));
		rows.push({kind: 'heading', group, count: entries.length}, ...sorted.map(entry => ({kind: 'worktree' as const, group, entry})));
	}
	if (!overview.entries.some(entry => !entry.isMain)) rows.push({kind: 'empty', text: 'No other worktrees: n then tab creates one with a new session'});
	return rows;
}

/** Rows j/k stop on: worktrees, and the heading of the ready group (x there deletes them all). */
export function selectableWorktreeRow(row: WorktreeRow): boolean {
	return row.kind === 'worktree' || (row.kind === 'heading' && row.group === 'ready');
}

export function worktreeRowKey(row: WorktreeRow): string | undefined {
	return row.kind === 'worktree' ? `wt:${row.entry.path}` : row.kind === 'heading' ? `group:${row.group}` : undefined;
}

/** Why x cannot delete it at all (shown instead of a confirmation), if so. */
export function deleteRefusal(entry: WorktreeOverviewEntry): string | undefined {
	if (entry.isMain) return 'The main checkout is never deleted';
	if (entry.inUse === 'this') return 'This Deckhand runs in that worktree; delete it from another checkout';
	if (entry.inUse === 'other') return 'Another Deckhand is open in that worktree; quit it first';
	if (entry.locked && !entry.missing) return 'That worktree is locked (git worktree unlock it first)';
	return undefined;
}

/** Branches never deleted with a worktree. */
export function protectedBranch(branch: string, defaultBranch?: string): boolean {
	return branch === 'main' || branch === 'master' || branch === defaultBranch;
}

/** The ready group's worktrees that x on its heading deletes (with their branches unless protected). */
export function bulkTargets(rows: WorktreeRow[]): WorktreeOverviewEntry[] {
	return rows.flatMap(row => (row.kind === 'worktree' && row.group === 'ready' && !deleteRefusal(row.entry) ? [row.entry] : []));
}

/** What the row says on its right: sessions, uncommitted work, commits of its own, age. Short first-dropped last. */
export function worktreeTags(entry: WorktreeOverviewEntry, sessions: SessionRecord[], now: number): string[] {
	const tags: string[] = [];
	if (entry.missing) tags.push(entry.missing === 'prunable' ? 'directory gone' : 'not listed by Git');
	if (entry.error) tags.push('not checked');
	const inspection = entry.inspection;
	if (inspection) {
		const changed = inspection.dirtyFiles + inspection.untrackedFiles;
		if (changed) tags.push(`${changed} changed`);
		if (inspection.ignoredFiles) tags.push(`${inspection.ignoredFiles} ignored`);
	}
	if (entry.aheadOfDefault) tags.push(`${entry.aheadOfDefault} commit${entry.aheadOfDefault === 1 ? '' : 's'}`);
	const running = entry.runningIds.length;
	if (running) tags.push(`${running} running`);
	else if (entry.sessionIds.length) tags.push(`${entry.sessionIds.length} session${entry.sessionIds.length === 1 ? '' : 's'}`);
	else if (!entry.isMain && !entry.recordId) tags.push('not from Deckhand');
	const active = lastActivity(entry, sessions);
	if (active !== undefined) tags.push(formatAge(now - active));
	return tags;
}
