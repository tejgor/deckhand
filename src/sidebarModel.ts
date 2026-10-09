import path from 'node:path';
import type {ProgramKey, SessionRecord} from './types.js';
import {sessionOutdated} from './agentVersions.js';
import {SESSION_FILTER_KEYS, SESSION_FILTERS, sessionNeedsAttention, type SessionFilter} from './sessionFeatures.js';
import {countHiddenSessionDescendants, countSessionDescendants, sessionDepth, sessionHasChildren} from './sessionOrder.js';
import {THEME, displaySessionTitle, programGlyph, statusColor, statusGlyph, truncate} from './ui.js';
import {wrapWords} from './menu.js';
import {workspaceKey} from './workspace.js';
import {openChecklistText} from './notes.js';

// Pure layout of the session sidebar (rendered by sidebar.tsx): rows, header and the selected session's details.

/** What a row segment is; sidebar.tsx maps roles to colors. */
export type RowRole = 'cursor' | 'gutter' | 'number' | 'tree' | 'status' | 'title' | 'gap' | 'dev' | 'archived' | 'cleanup' | 'merged' | 'done' | 'count' | 'outdated' | 'agent';
export interface RowPart {text: string; role: RowRole}
export interface SidebarRow {
	id: string;
	parts: RowPart[];
	selected: boolean;
	/** Archived rows outside the archived view, and context-only ancestors inside it. */
	dimmed: boolean;
	/** Marked done (D) and not dimmed: its title is slightly dimmed (muted), its markers stay readable. */
	done: boolean;
	/** statusColor of the session (status glyph, tree glyphs and title). */
	color: string;
}

export const GUTTER_MARKER = '╎';
// While the title would get fewer columns than this (or than it needs), suffix markers go in DROP_ORDER; the agent stays.
const MIN_TITLE = 4;
const DROP_ORDER: RowRole[] = ['outdated', 'count', 'merged', 'done', 'archived', 'cleanup', 'dev'];
export const DONE_MARKER = '☑';
export const OUTDATED_MARKER = '↑';

export interface SidebarRowsInput {
	/** The rows on screen, in order. */
	rows: SessionRecord[];
	allSessions: SessionRecord[];
	/** Row number of rows[0] (numbers are positions in the whole list, as the numeric jump keys use). */
	firstNumber: number;
	/** Digits of the largest number in the list. */
	numberWidth: number;
	selectedId?: string;
	/** Row width, the cursor column included (sidebar.tsx puts it in the border's padding column). */
	width: number;
	spinnerFrame: string;
	filter: SessionFilter;
	collapsedSessionIds?: ReadonlySet<string>;
	hiddenSessionIds?: ReadonlySet<string>;
	/** Installed agent versions: a running session launched with an older one gets ↑ (restart to update). */
	installedVersions?: Partial<Record<ProgramKey, string>>;
}

function isMerged(session: SessionRecord): boolean {
	return Boolean(session.worktree?.mergedAt || session.mergedAt);
}

/** Whether a row is dimmed: archived ones, except in the archived view, where the non-archived context ancestors are. */
export function rowDimmed(session: SessionRecord, filter: SessionFilter): boolean {
	return filter === 'archived' ? !session.archivedAt : Boolean(session.archivedAt);
}

export function sidebarRows({rows, allSessions, firstNumber, numberWidth, selectedId, width, spinnerFrame, filter, collapsedSessionIds = new Set(), hiddenSessionIds = new Set(), installedVersions = {}}: SidebarRowsInput): SidebarRow[] {
	const keys = new Map(rows.map(session => [session.id, workspaceKey(session)]));
	const selectedKey = rows.find(session => session.id === selectedId) ? keys.get(selectedId!) : undefined;
	// Dev belongs to the workspace: ▶ once, on its first row on screen.
	const devShown = new Set<string>();
	return rows.map((session, index) => {
		const selected = session.id === selectedId;
		const key = keys.get(session.id);
		const sharesSelected = !selected && selectedKey !== undefined && key === selectedKey;
		const depth = sessionDepth(session, allSessions);
		const hasChildren = sessionHasChildren(session.id, allSessions);
		const collapsed = collapsedSessionIds.has(session.id);
		const childCount = collapsed && hasChildren
			? countSessionDescendants(session.id, allSessions)
			: countHiddenSessionDescendants(session.id, allSessions, hiddenSessionIds);
		let dev = false;
		if (session.devRunning) {
			dev = key === undefined || !devShown.has(key);
			if (key !== undefined) devShown.add(key);
		}
		const tree = `${'  '.repeat(Math.min(depth, 4))}${hasChildren ? (collapsed ? '▸ ' : '▾ ') : ''}${session.subSessionKind === 'forked' ? '⑂ ' : session.subSessionKind === 'clean' ? '↳ ' : ''}`;
		const prefix: RowPart[] = [
			selected ? {text: '›', role: 'cursor'} : {text: sharesSelected ? GUTTER_MARKER : ' ', role: sharesSelected ? 'gutter' : 'cursor'},
			{text: ` ${String(firstNumber + index).padStart(numberWidth)} `, role: 'number'},
			...(tree ? [{text: tree, role: 'tree' as const}] : []),
			{text: `${statusGlyph(session, spinnerFrame)} `, role: 'status'},
		];
		const suffix: RowPart[] = [
			...(dev ? [{text: '▶', role: 'dev' as const}] : []),
			...(session.archivedAt ? [{text: '▣', role: 'archived' as const}] : []),
			...(session.cleanupError ? [{text: '!', role: 'cleanup' as const}] : []),
			...(isMerged(session) ? [{text: '✓', role: 'merged' as const}] : []),
			...(session.doneAt ? [{text: DONE_MARKER, role: 'done' as const}] : []),
			...(childCount > 0 ? [{text: `+${childCount}`, role: 'count' as const}] : []),
			...(sessionOutdated(session, installedVersions[session.program]) ? [{text: OUTDATED_MARKER, role: 'outdated' as const}] : []),
			{text: programGlyph(session.program), role: 'agent'},
		];
		const used = (parts: RowPart[]) => parts.reduce((sum, part) => sum + part.text.length, 0);
		const suffixWidth = () => used(suffix) + suffix.length - 1;
		const prefixWidth = used(prefix);
		const fullTitle = displaySessionTitle(session, allSessions);
		for (const role of DROP_ORDER) {
			if (width - prefixWidth - 1 - suffixWidth() >= Math.min(MIN_TITLE, fullTitle.length)) break;
			const at = suffix.findIndex(part => part.role === role);
			if (at >= 0) suffix.splice(at, 1);
		}
		const title = truncate(fullTitle, Math.max(0, width - prefixWidth - 1 - suffixWidth()));
		const gap = Math.max(1, width - prefixWidth - title.length - suffixWidth());
		const parts: RowPart[] = [...prefix, {text: title, role: 'title'}, {text: ' '.repeat(gap), role: 'gap'}];
		suffix.forEach((part, at) => parts.push(...(at ? [{text: ' ', role: 'gap' as const}] : []), part));
		const dimmed = !selected && rowDimmed(session, filter);
		return {id: session.id, parts: fitRow(parts, width), selected, dimmed, done: !selected && !dimmed && Boolean(session.doneAt), color: statusColor(session)};
	});
}

/** Parts cut to `width` columns in total, in order. */
function cutParts<T extends {text: string}>(parts: T[], width: number): T[] {
	const shown: T[] = [];
	let room = Math.max(0, width);
	for (const part of parts) {
		if (room <= 0) break;
		const text = part.text.length > room ? truncate(part.text, room) : part.text;
		if (text) shown.push({...part, text});
		room -= text.length;
	}
	return shown;
}

/** A row exactly `width` columns wide (cutting only bites when the prefix alone is too wide). */
function fitRow(parts: RowPart[], width: number): RowPart[] {
	const shown = cutParts(parts, width);
	const room = width - shown.reduce((sum, part) => sum + part.text.length, 0);
	return room > 0 ? [...shown, {text: ' '.repeat(room), role: 'gap'}] : shown;
}

export interface SidebarHeader {
	title: string;
	/** Filter and/or search with the count, or the plain count in the default view. */
	label: string;
	/** `! N` when non-archived sessions need attention, else empty. */
	attention: string;
	/** A non-default filter or a search is active. */
	highlighted: boolean;
}

/** Header text at `width` columns; the attention count wins over the label (count over filter/search), which wins over the title. */
export function sidebarHeader({width, filter, query, shown, total, allSessions}: {width: number; filter: SessionFilter; query: string; shown: number; total: number; allSessions: SessionRecord[]}): SidebarHeader {
	const needing = allSessions.filter(session => !session.archivedAt && sessionNeedsAttention(session)).length;
	const attention = needing > 0 ? `! ${needing}` : '';
	const search = query.trim();
	const highlighted = filter !== 'active' || Boolean(search);
	const tag = [filter !== 'active' ? filter : '', search ? `/${search}` : ''].filter(Boolean).join(' ');
	const count = highlighted ? `${shown}/${total}` : `${shown}`;
	const room = width - (attention ? attention.length + 3 : 0);
	let label = tag ? `${tag} ${count}` : count;
	if (label.length > room) {
		// Cut the filter/search, keep the count; below three columns of tag, drop it.
		const tagRoom = room - count.length - 1;
		label = tag && tagRoom >= 3 ? `${truncate(tag, tagRoom)} ${count}` : count;
	}
	// The title goes first when the label needs its columns.
	return {title: room - label.length - 1 >= 'Sessions'.length ? 'Sessions' : '', label, attention, highlighted};
}

/** One line of the filter menu (f): `› a  active      12`, the cursor in the sidebar's cursor column. */
export interface FilterMenuLine {filter: SessionFilter; selected: boolean; cursor: string; key: string; label: string; count: string}

/** The filter menu at `width` columns (the session rows' width): the label is cut before the count. */
export function filterMenuLines(selected: SessionFilter, counts: Record<SessionFilter, number>, width: number): FilterMenuLine[] {
	return SESSION_FILTERS.map(filter => {
		const cursor = filter === selected ? '›' : ' ', key = SESSION_FILTER_KEYS[filter], count = String(counts[filter]);
		// `› a  ` before the label, one column before the count.
		const room = width - 5 - count.length - 1;
		const label = room >= filter.length ? filter.padEnd(room) : truncate(filter, Math.max(0, room));
		return {filter, selected: filter === selected, cursor, key, label, count: room >= 0 ? ` ${count}` : ''};
	});
}

export interface DetailPart {text: string; color?: string; dim?: boolean}
export type DetailLine = DetailPart[];

/** The session's state in words, from the same inputs as statusGlyph. */
export function statusWords(session: SessionRecord): string {
	if (session.status === 'running') {
		switch (session.attention?.state) {
			case 'needs-input': return 'needs input';
			case 'response-ended': return 'response ended';
			case 'failed': return 'failed';
			case 'limited': return 'rate-limited';
			case 'working': return 'working';
		}
	}
	if (session.status === 'exited') return session.exitReason === 'failed' ? 'exited (failed)' : session.exitReason === 'interrupted' ? 'interrupted' : 'exited';
	if (session.status === 'starting') return 'starting';
	return session.agentStatus === 'active' ? 'working' : session.agentStatus === 'idle' ? 'idle' : 'running';
}

/** When the state statusWords describes began: the attention signal while running, else the last activity change (set on exit too). */
export function statusSince(session: SessionRecord): string | undefined {
	const attention = session.attention;
	if (session.status === 'running' && attention && attention.state !== 'unknown') return attention.at;
	return session.agentStatusUpdatedAt ?? session.updatedAt ?? session.createdAt;
}

const MINUTE = 60_000, HOUR = 60 * MINUTE, DAY = 24 * HOUR;
/** Compact age: `now`, `12m`, `3h`, `2d`. */
export function formatAge(ms: number): string {
	if (!(ms >= MINUTE)) return 'now';
	if (ms < HOUR) return `${Math.floor(ms / MINUTE)}m`;
	if (ms < DAY) return `${Math.floor(ms / HOUR)}h`;
	return `${Math.floor(ms / DAY)}d`;
}
/** Milliseconds until formatAge(ms) changes, so a clock can re-render exactly then. */
export function msUntilAgeChanges(ms: number): number {
	if (!(ms >= 0)) return MINUTE;
	const unit = ms < HOUR ? MINUTE : ms < DAY ? HOUR : DAY;
	return unit - (ms % unit) + 50;
}

function sessionBranch(session: SessionRecord, sharing: SessionRecord[]): string | undefined {
	return session.worktree?.branch || sharing.find(other => other.worktree?.branch && !other.worktree.isMain)?.worktree?.branch;
}

/** `done 2d ago` (`done now` within a minute; plain `done` without a clock or a readable time). */
export function doneText(session: SessionRecord, now?: number): string {
	const at = session.doneAt ? Date.parse(session.doneAt) : NaN;
	if (now === undefined || !Number.isFinite(at)) return 'done';
	const age = formatAge(now - at);
	return age === 'now' ? 'done now' : `done ${age} ago`;
}

/** Where the session runs plus its workspace markers, from session data only (no Git), cut to `width` at a ` · `. */
export function locationText(session: SessionRecord, allSessions: SessionRecord[], width = Infinity, now?: number): string {
	const key = workspaceKey(session);
	const sharing = key === undefined ? [] : allSessions.filter(other => other.id !== session.id && workspaceKey(other) === key);
	const worktree = session.worktree;
	let where: string;
	if (worktree?.deletedAt) where = 'worktree deleted';
	else if (key === undefined) where = 'preparing worktree';
	else if (worktree?.isMain || (!worktree?.id && (!worktree || worktree.mode === 'none'))) where = 'main checkout';
	else {
		const branch = sessionBranch(session, sharing);
		where = branch ? `⎇ ${branch}` : `worktree ${path.basename(key)}`;
	}
	const [first, ...rest] = [
		where,
		sharing.length ? `shared with ${sharing.length}` : '',
		session.devRunning ? '▶ dev' : '',
		isMerged(session) ? 'merged' : '',
		session.doneAt ? doneText(session, now) : '',
		session.archivedAt ? 'archived' : '',
	].filter(Boolean);
	let text = truncate(first!, width);
	for (const [index, part] of rest.entries()) {
		const next = `${text} · ${part}`;
		// Whole markers only; the ones cut leave a ` …` (so a marker is kept only if that still fits after it).
		if (next.length > width || (index < rest.length - 1 && next.length + 2 > width)) return text.length + 2 <= width ? `${text} …` : text;
		text = next;
	}
	return text;
}

/** Title lines: wrapped, at most `max`, the last one ending in … when cut. */
function titleLines(title: string, width: number, max: number): string[] {
	const lines = wrapWords(title, width);
	if (lines.length <= max) return lines;
	// The rest is wider than `width` (it wrapped), so truncate ends it in ….
	return [...lines.slice(0, max - 1), truncate(lines.slice(max - 1).join(' '), width)];
}

/**
 * The selected session's details in the `freeRows` rows the list leaves: a separator, the full title (two lines
 * with five free rows, else one), `agent · state · age`, where it runs, and open checklist items of its notes
 * (`☐ 3 open (2 worktree)`) when there are any. Fewer rows drop the checklist line first, then the location, and
 * fewer than three hide it.
 */
export function sessionDetails(session: SessionRecord | undefined, allSessions: SessionRecord[], width: number, freeRows: number, now: number, installedVersions: Partial<Record<ProgramKey, string>> = {}, task?: {title: string; done: boolean}): DetailLine[] {
	if (!session || freeRows < 3 || width < 1) return [];
	const title = displaySessionTitle(session, allSessions) || '(untitled)';
	const titles = titleLines(title, width, freeRows >= 5 ? 2 : 1);
	const since = statusSince(session);
	const sinceMs = since ? Date.parse(since) : NaN;
	const age = Number.isFinite(sinceMs) ? formatAge(now - sinceMs) : '';
	const state = statusWords(session);
	const lines: DetailLine[] = [
		[{text: '─'.repeat(width), color: THEME.border}],
		...titles.map(text => [{text, color: THEME.accentSoft}]),
		stateLine(session, state, age, width, installedVersions[session.program]),
	];
	if (freeRows - lines.length >= 1) lines.push([{text: locationText(session, allSessions, width, now), color: THEME.muted}]);
	// The task it works on (b): kept longer than the checklist line below.
	if (task && freeRows - lines.length >= 1) lines.push([{text: truncate(`◆ ${task.title}${task.done ? ' ✓' : ''}`, width), color: THEME.accentSoft}]);
	// Open checklist items of its notes and its worktree's: the first line to go when rows are short.
	const checklist = openChecklistText(session, width);
	if (checklist && freeRows - lines.length >= 1) lines.push([{text: checklist, color: THEME.muted}]);
	return lines;
}

/**
 * `<glyph> <agent> · <state> · <age>`; when too wide, the agent's name goes first (its glyph stays), then the age.
 * An outdated session names both versions first (`claude 2.1.287 · 2.1.290 installed`, then `2.1.287 → 2.1.290`).
 */
function stateLine(session: SessionRecord, state: string, age: string, width: number, installed?: string): DetailLine {
	const glyph = programGlyph(session.program);
	const sep = {text: ' · ', color: THEME.muted, dim: true};
	const stateText = {text: state, color: statusColor(session)};
	const ageParts = age ? [sep, {text: age, color: THEME.muted}] : [];
	const outdated = sessionOutdated(session, installed);
	const launched = session.agentVersion!;
	const versioned: DetailLine[] = outdated ? [
		[{text: `${glyph} ${session.program} ${launched}`, color: THEME.muted}, sep, {text: `${installed} installed`, color: THEME.muted}, sep, stateText, ...ageParts],
		[{text: `${glyph} ${session.program} ${launched} → ${installed}`, color: THEME.muted}, sep, stateText, ...ageParts],
		[{text: `${glyph} ${launched} → ${installed}`, color: THEME.muted}, sep, stateText, ...ageParts],
		[{text: `${glyph} ${launched} → ${installed}`, color: THEME.muted}, sep, stateText],
	] : [];
	const candidates: DetailLine[] = [
		...versioned,
		[{text: `${glyph} ${session.program}`, color: THEME.muted}, sep, stateText, ...ageParts],
		[{text: `${glyph} `, color: THEME.muted}, stateText, ...ageParts],
		[{text: `${glyph} `, color: THEME.muted}, stateText],
	];
	const lineWidth = (line: DetailLine) => line.reduce((sum, part) => sum + part.text.length, 0);
	return cutParts(candidates.find(line => lineWidth(line) <= width) ?? candidates.at(-1)!, width);
}
