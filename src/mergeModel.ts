import type {MergePreview, WorktreeMergeResult} from './types.js';
import {operationProblem} from './git.js';
import {fitHint} from './menu.js';
import {THEME, compactPath, truncate} from './ui.js';

// The merge confirmation (m) and the conflict result view as pure data; app.tsx renders them.

export interface MergeLine {text: string; color?: string; bold?: boolean}
export interface MergeNoteEntry {key: string; title: string; lines: string[]}

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;

export function mergeOptions(preview?: MergePreview): string[] {
	const into = preview?.targetBranch ?? 'the current branch';
	return [`Merge into ${into} without committing`, `Squash merge into ${into} without committing`, 'Cancel'];
}

/** The target is not what one would expect (not the main checkout, or not on the default branch): shown in warn. */
export function surprisingTarget(preview: MergePreview): boolean {
	return !preview.targetIsMain || !preview.targetBranch || (preview.defaultBranch !== undefined && preview.targetBranch !== preview.defaultBranch);
}

/** Target files with uncommitted changes the merge would touch (the source's uncommitted files only when they are committed first). */
export function overlappingFiles(preview: MergePreview, commitFirst: boolean): string[] {
	return [...new Set([...preview.overlap.committed, ...commitFirst && preview.uncommitted ? preview.overlap.uncommitted : []])];
}

/** Follow-ups the confirmation lists (and lets you tick); the heading counts the rest, which go back to the backlog. */
export const MERGE_FOLLOW_UPS = 5;

export interface MergeConfirmInput {
	/** The session's display title (the screen's title). */
	title: string;
	preview?: MergePreview;
	previewError?: string;
	/** Commit the source worktree's uncommitted files first (the toggle; on by default). */
	commitFirst: boolean;
	/** The commit message that would be used: the session's title. */
	commitMessage: string;
	/** A merge attempt that failed (e.g. the commit's hook output); nothing was merged. */
	error?: string;
	notes: MergeNoteEntry[];
	/** Open tasks the worktree was started for: merging ticks them. */
	tasks?: string[];
	/** Open tasks assigned to the worktree (follow-ups): merging sends them back to the backlog unless ticked here. */
	followUps?: Array<{title: string; ticked: boolean}>;
	/** Open checklist items of the worktree's note: sent to the backlog with the merge unless `send` is switched off. */
	openItems?: {titles: string[]; send: boolean};
	/** Content columns (inside the border and padding). */
	width: number;
	/** The pane's rows, border included. */
	height: number;
}
export interface MergeConfirmLayout {
	title: string;
	/** Target, warnings, summary, commits and the uncommitted toggle: under the title. */
	details: MergeLine[];
	/** One row per follow-up (selectable, between the details and the options), after a heading. */
	followUps: string[];
	options: string[];
	hint: string;
	/** A failed attempt's output, after the hint. */
	error: MergeLine[];
	/** The Notes section (its header first), last; empty when it does not fit. */
	notes: MergeLine[];
}

/**
 * The confirmation screen in `height` rows: title, details, a blank, the three options, a blank, the hint, then a
 * failed attempt's output and the notes. When rows are short the notes shrink first (and go), then the error output,
 * then the commit subjects; the target line, warnings, summary and toggle stay.
 */
export function mergeConfirmLayout(input: MergeConfirmInput): MergeConfirmLayout {
	const {preview, width} = input;
	const fit = (text: string) => truncate(text, width);
	const head: MergeLine[] = [];
	const tail: MergeLine[] = [];
	let commits: MergeLine[] = [];
	if (!preview) head.push(input.previewError ? {text: fit(`Preview unavailable: ${input.previewError}`), color: THEME.warn} : {text: 'Checking what will be merged…', color: THEME.muted});
	else {
		const branch = preview.targetBranch ?? '(detached)';
		const prefix = `Into ${branch} · `;
		head.push({text: fit(`${prefix}${compactPath(preview.targetRoot, Math.max(8, width - prefix.length))}`), color: surprisingTarget(preview) ? THEME.warn : THEME.muted});
		if (preview.inProgress) head.push({text: fit(`⚠ ${operationProblem(preview.inProgress)}`), color: THEME.warn});
		const extra = input.commitFirst && preview.uncommitted ? ` · +${plural(preview.uncommitted, 'uncommitted file')}` : '';
		head.push(preview.commitCount
			? {text: fit(`${plural(preview.commitCount, 'commit')} · ${plural(preview.diff.files, 'file')} +${preview.diff.insertions} −${preview.diff.deletions}${extra}`)}
			: {text: fit(`No new commits${extra}`), color: extra ? undefined : THEME.muted});
		commits = preview.commits.map(subject => ({text: fit(`  ${subject}`), color: THEME.muted}));
		const more = preview.commitCount - preview.commits.length;
		if (commits.length && more > 0) commits.push({text: fit(`  +${more} more`), color: THEME.muted});
		if (preview.uncommitted) {
			tail.push({text: fit(plural(preview.uncommitted, 'uncommitted file'))});
			// The message gives way first, so the line always says what happens.
			const toggle = (quoted: string) => input.commitFirst ? `☑ commit them first (${quoted})` : `☐ commit them first (${quoted}) · they stay in the worktree`;
			const quoted = `"${truncate(input.commitMessage, Math.max(4, width - toggle('""').length))}"`;
			tail.push({text: fit(toggle(quoted)), color: input.commitFirst ? THEME.active : THEME.muted});
		}
		const tasks = input.tasks ?? [];
		if (tasks.length === 1) tail.push({text: fit(`◆ Task "${tasks[0]}" will be marked done`), color: THEME.accentSoft});
		else if (tasks.length > 1) tail.push({text: fit(`◆ ${tasks.length} tasks will be marked done: ${tasks.join(', ')}`), color: THEME.accentSoft});
		const overlap = overlappingFiles(preview, input.commitFirst);
		if (overlap.length) tail.push({text: fit(`⚠ Target has uncommitted changes in ${plural(overlap.length, 'file')} the merge touches: ${overlap.slice(0, 3).join(', ')}${overlap.length > 3 ? ', …' : ''}`), color: THEME.warn});
	}
	const listed = preview ? input.followUps ?? [] : [];
	const followUps = listed.slice(0, MERGE_FOLLOW_UPS).map(item => fit(`${item.ticked ? '☑' : '☐'} ${item.title} · ${item.ticked ? 'done with this merge' : 'back to the backlog'}`));
	const unlisted = listed.length - followUps.length;
	// The note's open items: one more selectable row after the follow-ups (space switches it).
	const items = preview ? input.openItems : undefined;
	if (items?.titles.length) {
		const count = plural(items.titles.length, 'open note item');
		followUps.push(fit(`${items.send ? '☑' : '☐'} ${count} ${items.send ? '→ backlog' : '· stay in the note'}: ${items.titles.join(', ')}`));
	}
	if (followUps.length > (items?.titles.length ? 1 : 0)) tail.push({text: fit(unlisted ? `Follow-ups · space ticks finished ones · +${unlisted} more go back` : 'Follow-ups · space ticks the ones you finished'), color: THEME.accentSoft});
	// Space toggles the selected follow-up, else commit-first: one hint for both.
	const space = items?.titles.length ? {text: 'space toggle', short: 'space toggle'}
		: preview?.uncommitted && followUps.length ? {text: 'space commit first or tick', short: 'space toggle'}
		: preview?.uncommitted ? {text: 'space commit first', short: 'space toggle'} : followUps.length ? {text: 'space tick a follow-up', short: 'space tick'} : undefined;
	const hint = fitHint(['enter choose', ...space ? [space] : [], {text: 'j/k move', drop: 1}, 'esc cancel'], width);
	// Border (2), title, the blank and three options, the blank and the hint.
	let free = Math.max(0, input.height - 2 - 1 - 4 - 2 - head.length - tail.length - followUps.length);
	const take = (lines: MergeLine[], reserve = 0) => { const shown = lines.slice(0, Math.max(0, free - reserve)); free -= shown.length; return shown; };
	// Commit subjects before the error and the notes; when cut, the last row shown says how many more there are.
	let shownCommits = take(commits);
	if (shownCommits.length < commits.length && shownCommits.length > 0) {
		const hidden = preview!.commitCount - (shownCommits.length - 1);
		shownCommits = [...shownCommits.slice(0, -1), {text: fit(`  +${hidden} more`), color: THEME.muted}];
	}
	const errorLines = input.error ? input.error.split('\n').map(line => line.trimEnd()).filter(Boolean).map((line, index) => ({text: fit(index ? `  ${line}` : line), color: THEME.error})) : [];
	// A blank row before the error output, then its lines (the last ones matter most for a hook's failure).
	const error = errorLines.length && free >= 2 ? (() => { free--; const lines = errorLines.length <= free ? errorLines : [errorLines[0]!, ...errorLines.slice(-(free - 1))].slice(0, free); free -= lines.length; return lines; })() : [];
	// The notes: a blank, the header and at least one line, else nothing.
	const noteLines: MergeLine[] = input.notes.length
		? input.notes.flatMap(entry => [{text: fit(entry.title), color: THEME.muted, bold: true}, ...entry.lines.map(line => ({text: fit(`  ${line}`), color: THEME.muted}))])
		: [{text: fit('No notes for this worktree, this session or its sub-sessions.'), color: THEME.muted}];
	let notes: MergeLine[] = [];
	if (free >= 3) {
		const room = free - 2;
		const shown = noteLines.length <= room ? noteLines : [...noteLines.slice(0, room - 1), {text: `+${noteLines.length - room + 1} more lines`, color: THEME.muted}];
		notes = [{text: 'Notes', color: THEME.accentSoft, bold: true}, ...shown];
		free -= notes.length + 1;
	}
	return {title: fit(`Merge "${input.title}"?`), details: [...head, ...shownCommits, ...tail], followUps, options: mergeOptions(preview), hint, error, notes};
}

/** Rows the confirmation needs, border included (for tests: never more than the pane). */
export function mergeConfirmRows(layout: MergeConfirmLayout): number {
	return 2 + 1 + layout.details.length + layout.followUps.length + 1 + layout.options.length + 1 + 1 + (layout.error.length ? layout.error.length + 1 : 0) + (layout.notes.length ? layout.notes.length + 1 : 0);
}

export const CONFLICT_FILES_SHOWN = 5;
export interface ConflictView {title: string; files: string[]; choices: Array<{key: string; text: string}>}
/** The result of a conflicted merge: what conflicts, and the only two choices (keep it in progress, or abort it). */
export function conflictView(result: Pick<WorktreeMergeResult, 'conflicts' | 'conflictCount'>, width: number): ConflictView {
	const files = result.conflicts ?? [];
	const total = Math.max(result.conflictCount ?? files.length, files.length);
	const shown = files.slice(0, total > CONFLICT_FILES_SHOWN ? CONFLICT_FILES_SHOWN - 1 : CONFLICT_FILES_SHOWN);
	return {
		title: truncate(`Merged with conflicts in ${plural(total, 'file')}`, width),
		files: [...shown.map(file => truncate(`  ${file}`, width)), ...total > shown.length ? [`  +${total - shown.length} more`] : []],
		choices: [
			{key: 'enter', text: 'keep it: resolve it in your editor or the Git tab (esc too)'},
			{key: 'x', text: 'abort the merge'},
		],
	};
}
