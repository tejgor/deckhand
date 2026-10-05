import type {StatusEntry} from './workspaceGit.js';

// The Git tab's Changes view as pure data (unit-tested): VS Code-style groups built from porcelain-v2 status plus
// numstat, the rows and selection the pane shows, and the diff preview's line classification and first changed line.
// Git I/O lives in changesGit.ts; the daemon polls it per workspace (workspace.ts) and pushes ChangesRecords.

export type ChangeGroup = 'conflicts' | 'staged' | 'unstaged' | 'untracked';
/** VS Code's order. */
export const CHANGE_GROUPS: readonly ChangeGroup[] = ['conflicts', 'staged', 'unstaged', 'untracked'];
export const CHANGE_GROUP_TITLES: Record<ChangeGroup, string> = {conflicts: 'Merge Conflicts', staged: 'Staged Changes', unstaged: 'Changes', untracked: 'Untracked'};
/** Entries a ChangesRecord carries at most (in group order); each group still reports its full count. */
export const MAX_CHANGES = 2000;

export interface LineCounts {additions?: number; deletions?: number; binary?: boolean}
export interface ChangeEntry extends LineCounts {
	group: ChangeGroup;
	/** Repository-relative path (the new path of a rename/copy). */
	path: string;
	/** A staged rename's or copy's source. */
	origPath?: string;
	/** One letter: M, A, D, R, C, T, U (conflict) or ? (untracked). */
	status: string;
	/** A conflict's two-letter code (UU, AA, DU, …). */
	conflict?: string;
}
export type ChangeCounts = Record<ChangeGroup, number>;
/** The Changes view of one workspace; `sessionId`/`workspace` are stamped by the daemon like the other workspace panes. */
export interface ChangesRecord {
	sessionId?: string;
	/** The workspace (worktree root) these changes belong to; absent while the session has none. */
	workspace?: string;
	/** False until the first status read finished. */
	loaded: boolean;
	branch?: string;
	/** Group order, sorted by path within a group, capped at MAX_CHANGES. */
	entries: ChangeEntry[];
	/** Every entry per group, including those left out by the cap. */
	counts: ChangeCounts;
	/** Entries per group left out by the cap. */
	omitted: ChangeCounts;
	error?: string;
}
/** One entry's diff preview (`changes-diff`). Untracked files are rendered as an all-additions diff. */
export interface ChangeDiff {
	group: ChangeGroup;
	path: string;
	text: string;
	/** Output was cut at the byte/line bound. */
	truncated: boolean;
	binary: boolean;
	/** First changed line on the new side (1 when there is none), for opening the file in an editor. */
	firstLine: number;
}

export const emptyCounts = (): ChangeCounts => ({conflicts: 0, staged: 0, unstaged: 0, untracked: 0});
export const emptyChanges = (fields: Partial<ChangesRecord> = {}): ChangesRecord => ({loaded: false, entries: [], counts: emptyCounts(), omitted: emptyCounts(), ...fields});

/** `git diff --numstat -z` by (new) path; a path listed twice (a conflict's combined diff) keeps its last record. */
export function parseNumstat(raw: string): Map<string, LineCounts> {
	const counts = new Map<string, LineCounts>();
	const records = raw.split('\0');
	for (let index = 0; index < records.length; index++) {
		const match = records[index]!.match(/^(\d+|-)\t(\d+|-)\t([\s\S]*)$/);
		if (!match) continue;
		let file = match[3]!;
		// A rename: empty path, then the old and new paths.
		if (!file) { file = records[index + 2] ?? ''; index += 2; }
		counts.set(file, match[1] === '-' ? {binary: true} : {additions: Number(match[1]), deletions: Number(match[2])});
	}
	return counts;
}

const byPath = (left: ChangeEntry, right: ChangeEntry) => (left.path < right.path ? -1 : left.path > right.path ? 1 : 0);
const letter = (code: string) => code === '?' ? '?' : code;

/**
 * VS Code's groups from porcelain-v2 entries: unmerged → Merge Conflicts; the index side (X) → Staged Changes; the
 * worktree side (Y) → Changes; untracked → Untracked. A partially staged file is in both Staged and Changes.
 * Line counts come from `git diff --cached --numstat` (staged), `git diff --numstat` (unstaged, conflicts) and the
 * file itself (untracked).
 */
export function groupChanges(entries: StatusEntry[], numstat: {staged?: Map<string, LineCounts>; unstaged?: Map<string, LineCounts>; untracked?: Map<string, LineCounts>} = {}, cap = MAX_CHANGES): Pick<ChangesRecord, 'entries' | 'counts' | 'omitted'> {
	const groups: Record<ChangeGroup, ChangeEntry[]> = {conflicts: [], staged: [], unstaged: [], untracked: []};
	for (const entry of entries) {
		if (entry.kind === '?') { groups.untracked.push({group: 'untracked', path: entry.path, status: '?', ...numstat.untracked?.get(entry.path)}); continue; }
		if (entry.kind === 'u') { groups.conflicts.push({group: 'conflicts', path: entry.path, status: 'U', conflict: entry.xy, ...numstat.unstaged?.get(entry.path)}); continue; }
		const [index = '.', worktree = '.'] = entry.xy;
		if (index !== '.') groups.staged.push({group: 'staged', path: entry.path, status: letter(index), ...(entry.origPath !== undefined && /[RC]/.test(index) ? {origPath: entry.origPath} : {}), ...numstat.staged?.get(entry.path)});
		if (worktree !== '.') groups.unstaged.push({group: 'unstaged', path: entry.path, status: letter(worktree), ...numstat.unstaged?.get(entry.path)});
	}
	const counts = emptyCounts(), omitted = emptyCounts(), shown: ChangeEntry[] = [];
	let room = Math.max(0, cap);
	for (const group of CHANGE_GROUPS) {
		const list = groups[group].sort(byPath);
		counts[group] = list.length;
		const take = Math.min(room, list.length);
		shown.push(...list.slice(0, take));
		omitted[group] = list.length - take;
		room -= take;
	}
	return {entries: shown, counts, omitted};
}

export const changeKey = (entry: Pick<ChangeEntry, 'group' | 'path'>) => `${entry.group}\0${entry.path}`;
export const findChange = (entries: readonly ChangeEntry[], group: ChangeGroup, file: string) => entries.find(entry => entry.group === group && entry.path === file);
export const totalChanges = (counts: ChangeCounts) => CHANGE_GROUPS.reduce((sum, group) => sum + counts[group], 0);

export type ChangeRow =
	| {kind: 'header'; group: ChangeGroup; count: number}
	| {kind: 'entry'; entry: ChangeEntry; /** Index among the selectable entries (`record.entries`). */ index: number}
	| {kind: 'more'; group: ChangeGroup; count: number};
/** The list as rows: a header per non-empty group, its entries, and "+N more" where the cap cut it. */
export function changeRows(record: Pick<ChangesRecord, 'entries' | 'counts' | 'omitted'>): ChangeRow[] {
	const rows: ChangeRow[] = [];
	let index = 0;
	for (const group of CHANGE_GROUPS) {
		if (!record.counts[group]) continue;
		rows.push({kind: 'header', group, count: record.counts[group]});
		while (index < record.entries.length && record.entries[index]!.group === group) { rows.push({kind: 'entry', entry: record.entries[index]!, index}); index++; }
		if (record.omitted[group]) rows.push({kind: 'more', group, count: record.omitted[group]});
	}
	return rows;
}

/**
 * The selection after a refresh: the same (group, path) when it is still listed; else the entry now at the same
 * position within that group (so staging a file selects the next one in its group), or its last entry; when the group
 * emptied, the first entry below it (else the last one). -1 when there is nothing to select.
 */
export function reselect(entries: readonly ChangeEntry[], previous: {key?: string; index: number; offset?: number}): number {
	if (!entries.length) return -1;
	const same = previous.key === undefined ? -1 : entries.findIndex(entry => changeKey(entry) === previous.key);
	if (same >= 0) return same;
	const group = previous.key?.split('\0')[0];
	const start = entries.findIndex(entry => entry.group === group);
	if (start >= 0) {
		let end = start;
		while (entries[end + 1]?.group === group) end++;
		return Math.min(end, start + Math.max(0, previous.offset ?? 0));
	}
	// The group emptied: the first entry of the next non-empty group below it, else the last one above it.
	const order = CHANGE_GROUPS.indexOf(group as ChangeGroup);
	if (order >= 0) {
		const below = entries.findIndex(entry => CHANGE_GROUPS.indexOf(entry.group) > order);
		return below >= 0 ? below : entries.length - 1;
	}
	return Math.max(0, Math.min(entries.length - 1, previous.index));
}
/** An entry's position within its group (for reselect). */
export function groupOffset(entries: readonly ChangeEntry[], index: number): number {
	const group = entries[index]?.group;
	let offset = 0;
	while (index - offset - 1 >= 0 && entries[index - offset - 1]!.group === group) offset++;
	return offset;
}

/** What space does with an entry: unstaged, untracked and conflicted entries are staged; staged ones unstaged. */
export function stageMode(entry: Pick<ChangeEntry, 'group'>): 'stage' | 'unstage' {
	return entry.group === 'staged' ? 'unstage' : 'stage';
}

/** Name and directory for display, VS Code style; a rename reads `old → new` (names only when in one directory). */
export function changeLabel(entry: Pick<ChangeEntry, 'path' | 'origPath'>): {name: string; dir: string} {
	const split = (file: string) => { const trimmed = file.replace(/\/$/, ''), cut = trimmed.lastIndexOf('/'); return {name: `${trimmed.slice(cut + 1)}${file.endsWith('/') ? '/' : ''}`, dir: cut >= 0 ? trimmed.slice(0, cut) : ''}; };
	const target = split(entry.path);
	if (entry.origPath === undefined) return target;
	const source = split(entry.origPath);
	return source.dir === target.dir ? {name: `${source.name} → ${target.name}`, dir: target.dir} : {name: `${entry.origPath} → ${entry.path}`, dir: ''};
}

/** `+12 −3`, `bin` for binary files, empty when unknown. */
export function lineCountsText(counts: LineCounts): string {
	if (counts.binary) return 'bin';
	if (counts.additions === undefined || counts.deletions === undefined) return '';
	return `+${counts.additions} −${counts.deletions}`;
}

const HUNK_PATTERN = /^(@{2,}) (?:-\d+(?:,\d+)? )+\+(\d+)(?:,(\d+))? @{2,}/;

/**
 * The first changed line on the new side of a unified (or combined, `@@@`) diff: walks the first hunk's lines,
 * counting new-side lines until the first added or removed one. 1 when there is no hunk.
 */
export function firstChangedLine(diff: string): number {
	const lines = diff.split('\n');
	const start = lines.findIndex(line => HUNK_PATTERN.test(line));
	if (start < 0) return 1;
	const [, ats, from] = lines[start]!.match(HUNK_PATTERN)!;
	const parents = ats!.length - 1;
	let line = Number(from);
	for (const text of lines.slice(start + 1)) {
		if (HUNK_PATTERN.test(text) || text.startsWith('diff ')) break;
		if (text.startsWith('\\')) continue;
		const prefix = text.slice(0, parents);
		if (/[+-]/.test(prefix)) break;
		line++;
	}
	return Math.max(1, line);
}

export type DiffLineKind = 'add' | 'del' | 'hunk' | 'meta' | 'context';
export interface DiffLine {kind: DiffLineKind; text: string}
/**
 * Classifies each diff line for colouring. Header lines (`diff`, `index`, `---`/`+++`, modes, renames) count only
 * before a file's first hunk; a combined diff's (conflict) prefixes span one column per parent.
 */
export function classifyDiff(text: string): DiffLine[] {
	let parents = 1, header = true;
	return text.replace(/\n$/, '').split('\n').map(line => {
		const hunk = line.match(HUNK_PATTERN);
		if (hunk) { parents = hunk[1]!.length - 1; header = false; return {kind: 'hunk', text: line}; }
		if (line.startsWith('diff ')) { header = true; return {kind: 'meta', text: line}; }
		if (header) return {kind: 'meta', text: line};
		const prefix = line.slice(0, parents);
		return {kind: prefix.includes('-') ? 'del' : prefix.includes('+') ? 'add' : 'context', text: line};
	});
}

/** An untracked file's contents as an all-additions diff (so it renders and opens like any other entry). */
export function untrackedDiff(content: string): string {
	const body = content.replace(/\n$/, '');
	const lines = body ? body.split('\n') : [];
	return [`@@ -0,0 +1,${lines.length} @@`, ...lines.map(line => `+${line}`)].join('\n');
}
