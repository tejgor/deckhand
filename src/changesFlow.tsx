import React, {useEffect, useRef, useState} from 'react';
import {existsSync} from 'node:fs';
import path from 'node:path';
import type {Key} from 'ink';
import type {LiveClient} from './client.js';
import type {SessionRecord} from './types.js';
import {changeKey, changeRows, groupOffset, reselect, stageMode, type ChangeEntry, type ChangesRecord} from './changesModel.js';
import {ChangesPane, changesLayout, diffLines, type DiffState} from './changesPane.js';
import {openInEditor} from './desktop.js';
import {errorMessage} from './ui.js';

// The Git tab's Changes view: selection (kept by group + path across refreshes), the selected entry's diff, and the
// focus-mode keys (v). The record itself is watched by the app like the other panes (watch-changes, changes-updated).

const DIFF_DEBOUNCE_MS = 60;
const DIFF_STEP = 3;

interface ChangesFlowOptions {
	client?: LiveClient;
	session?: SessionRecord;
	changes: ChangesRecord;
	focused: boolean;
	/** A refreshed record from a stage/unstage response. */
	onChanges: (changes: ChangesRecord) => void;
	onExit: () => void;
	/** o: attach the workspace's lazygit. */
	onAttach: () => void;
	setBusy: (busy: boolean) => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
}
export interface ChangesFlow {
	handleInput(input: string, key: Partial<Key>): void;
	render(width: number, height: number): React.ReactNode;
}

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;

export function useChangesFlow({client, session, changes, focused, onChanges, onExit, onAttach, setBusy, setError, setStatusMessage}: ChangesFlowOptions): ChangesFlow {
	const [selection, setSelection] = useState<{key?: string; index: number; offset?: number}>({index: 0});
	const [diff, setDiff] = useState<DiffState>();
	const [diffScroll, setDiffScroll] = useState(0);
	const viewport = useRef({diffRows: 10});
	const current = changes.sessionId === session?.id && changes.workspace ? changes : undefined;
	const entries = current?.entries ?? [];
	const selected = reselect(entries, selection);
	const entry: ChangeEntry | undefined = selected >= 0 ? entries[selected] : undefined;
	const key = entry ? changeKey(entry) : undefined;

	// Follow the entry that is now selected (e.g. the next one after staging the last selection away).
	useEffect(() => {
		if (key !== selection.key || selected !== selection.index) setSelection({key, index: Math.max(0, selected), offset: groupOffset(entries, selected)});
	}, [key, selected]);
	useEffect(() => { setDiffScroll(0); }, [key]);
	useEffect(() => { setSelection({index: 0}); setDiff(undefined); }, [session?.id]);

	// The selected entry's diff, while focused; refetched when the record changes (pushes happen only on changes).
	useEffect(() => {
		if (!client || !focused || !session || !entry || !key) return;
		let cancelled = false;
		const timer = setTimeout(() => {
			void client.changesDiff(session.id, entry.group, entry.path).then(
				next => { if (!cancelled) setDiff({key, diff: next}); },
				error => { if (!cancelled) setDiff({key, error: errorMessage(error)}); },
			);
		}, DIFF_DEBOUNCE_MS);
		return () => { cancelled = true; clearTimeout(timer); };
	}, [client, focused, session?.id, key, current]);

	const move = (index: number) => {
		if (!entries.length) return;
		const next = Math.max(0, Math.min(entries.length - 1, index));
		setSelection({key: changeKey(entries[next]!), index: next, offset: groupOffset(entries, next)});
	};
	const scrollDiff = (delta: number) => {
		const lines = diff?.key === key ? diffLines(diff?.diff).length : 0;
		setDiffScroll(scroll => Math.max(0, Math.min(Math.max(0, lines - viewport.current.diffRows), scroll + delta)));
	};

	const stage = (mode: 'stage' | 'unstage', target?: ChangeEntry) => {
		if (!client || !session) return;
		setBusy(true); setError(undefined);
		void client.changeStage(session.id, mode, target && {group: target.group, path: target.path}).then(result => {
			onChanges(result.changes);
			const what = target ? target.path : plural(result.changed, 'file');
			setStatusMessage(!target && !result.changed ? `Nothing to ${mode}` : `${mode === 'stage' ? 'Staged' : 'Unstaged'} ${what}${result.skippedConflicts ? ` · ${plural(result.skippedConflicts, 'conflicted file')} left: stage each with space once resolved` : ''}`);
		}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
	};

	// Opens the file at the first changed line of its diff (the new side); a deleted file cannot be opened.
	const open = async () => {
		if (!client || !session || !entry || !current?.workspace) return;
		const file = path.join(current.workspace, entry.path);
		if (!existsSync(file)) { setStatusMessage(`${entry.path} is deleted; nothing to open (o opens lazygit)`); return; }
		let line = 1;
		if (entry.group !== 'untracked') {
			const loaded = diff && diff.key === key && diff.diff ? diff.diff : await client.changesDiff(session.id, entry.group, entry.path).catch(() => undefined);
			line = loaded?.firstLine ?? 1;
		}
		const label = openInEditor(file, setError, line);
		if (label) setStatusMessage(`Opened ${entry.path}:${line} in ${label}`);
	};

	const handleInput = (input: string, key: Partial<Key>) => {
		if (key.escape || input === 'v') { onExit(); return; }
		if (input === 'o') { onAttach(); return; }
		if (key.downArrow || input === 'j') { move(selected + 1); return; }
		if (key.upArrow || input === 'k') { move(selected - 1); return; }
		if (key.home || input === 'g') { move(0); return; }
		if (key.end || input === 'G') { move(entries.length - 1); return; }
		if (input === 'J') { scrollDiff(DIFF_STEP); return; }
		if (input === 'K') { scrollDiff(-DIFF_STEP); return; }
		if (key.pageDown) { scrollDiff(Math.max(1, viewport.current.diffRows - 1)); return; }
		if (key.pageUp) { scrollDiff(-Math.max(1, viewport.current.diffRows - 1)); return; }
		if (!current) return;
		if (input === ' ') { if (entry) stage(stageMode(entry), entry); return; }
		if (input === 'a') { stage('stage'); return; }
		if (input === 'A') { stage('unstage'); return; }
		if (key.return || input === 'E') { void open().catch(error => setError(errorMessage(error))); }
	};

	const render = (width: number, height: number) => {
		viewport.current.diffRows = changesLayout(width, height, current ? changeRows(current).length : 0).diffRows;
		return <ChangesPane session={session} changes={changes} focused={focused} selected={selected} diff={diff?.key === key ? diff : undefined} diffScroll={diffScroll} width={width} height={height} />;
	};

	return {handleInput, render};
}
