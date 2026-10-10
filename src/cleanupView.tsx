import React from 'react';
import {Text} from 'ink';
import type {CleanupFile, CleanupInspection} from './types.js';
import {THEME} from './ui.js';

// What deleting a worktree would lose, as the deletion confirmations show it (x on a session, x in W, and their typed
// DELETE screens): every reason on a line of its own, then the files themselves. A one-line summary cut at the pane's
// width named at most a count and three ignored files, which defeated the point of flagging them.

export const CLEANUP_FILE_MARKS: Record<CleanupFile['state'], string> = {changed: 'M', untracked: '?', ignored: '!'};
export const CLEANUP_FILE_LEGEND = 'M changed · ? untracked · ! ignored';

/** Every file the inspection counted (its `files` list may be cut). */
export function lostFileCount(inspection: Pick<CleanupInspection, 'files' | 'dirtyFiles' | 'untrackedFiles' | 'ignoredFiles'>): number {
	return Math.max(inspection.files?.length ?? 0, inspection.dirtyFiles + inspection.untrackedFiles + inspection.ignoredFiles);
}

/** A path cut from the front (the file's name is the end of it). */
export function fitPath(file: string, width: number): string {
	return file.length <= width ? file : `…${file.slice(file.length - Math.max(1, width - 1))}`;
}

/** The files in at most `rows` rows (the last one says `+N more` when they do not all fit), as `M path` lines. */
export function lostFileRows(inspection: CleanupInspection, rows: number): {files: CleanupFile[]; more: number} {
	const files = inspection.files ?? [];
	const total = lostFileCount(inspection);
	const shown = total > rows ? files.slice(0, Math.max(0, rows - 1)) : files.slice(0, rows);
	return {files: shown, more: total - shown.length};
}

/** The reasons (wrapped, never cut) and the files in at most `fileRows` rows, with a legend above them. */
export function cleanupLossLines(inspection: CleanupInspection, width: number, fileRows: number): React.ReactNode[] {
	const {files, more} = lostFileRows(inspection, Math.max(1, fileRows));
	const listed = files.length + (more > 0 ? 1 : 0) > 0;
	return [
		...inspection.reasons.map((reason, index) => <Text key={`reason-${index}`} color={THEME.warn} wrap="wrap">{`· ${reason}`}</Text>),
		...listed && files.length ? [<Text key="legend" color={THEME.muted} wrap="truncate-end">{`  Files (${CLEANUP_FILE_LEGEND}):`}</Text>] : [],
		...files.map((file, index) => (
			<Text key={`file-${index}`} wrap="truncate-end"><Text color={THEME.warn}>{`    ${CLEANUP_FILE_MARKS[file.state]} `}</Text>{fitPath(file.path, Math.max(4, width - 6))}</Text>
		)),
		...more > 0 && files.length ? [<Text key="more" color={THEME.muted}>{`    +${more} more`}</Text>] : [],
	];
}

/** Rows a confirmation can give the file list: what is left of `height` after `used` rows (at least three). */
export function fileRowsLeft(height: number, used: number): number {
	return Math.max(3, height - used);
}
