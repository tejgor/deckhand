import React, {useRef, useState} from 'react';
import path from 'node:path';
import {Box, Text, type Key} from 'ink';
import type {LiveClient} from './client.js';
import type {SessionRecord, WorktreeCleanupInspection, WorktreeOverview, WorktreeOverviewEntry} from './types.js';
import {GROUP_TITLES, bulkTargets, deleteRefusal, mergedText, protectedBranch, selectableWorktreeRow, worktreeName, worktreeRowKey, worktreeRows, worktreeTags, type WorktreeRow} from './worktreesModel.js';
import {leadSession} from './tasksBoard.js';
import {openInEditor} from './desktop.js';
import {SelectableRow, fitHint, scrolledListTop} from './menu.js';
import {cleanupLossLines, fileRowsLeft} from './cleanupView.js';
import {formatAge} from './sidebarModel.js';
import {THEME, compactPath, displaySessionTitle, errorMessage, statusColor, statusGlyph, truncate} from './ui.js';

// W, the worktree manager: every worktree of the repository (Git's list plus Deckhand's records), grouped by what to do
// with it (src/worktreesModel.ts), with its sessions, merge state and what deleting it would lose. x deletes one (or, on
// the ready group's heading, all of them) after a confirmation that names the sessions it stops; data loss still needs
// a typed DELETE, as with x on a session. The daemon re-checks everything at deletion time.

const DETAIL_ROWS = 5;

type ConfirmKind = 'delete' | 'delete-branch' | 'prune' | 'forget' | 'cancel';
interface ConfirmOption {kind: ConfirmKind; label: string}

interface DeleteConfirm {
	/** One worktree, or the ready group (`bulk`). */
	entries: WorktreeOverviewEntry[];
	bulk: boolean;
	index: number;
	/** The daemon's fresh check of the worktree alone and with its branch (one worktree only). */
	checks: {worktree?: WorktreeCleanupInspection; branch?: WorktreeCleanupInspection};
	/** Typing DELETE to authorize data loss for this option. */
	override?: {deleteBranch: boolean; draft: string};
}

interface WorktreesFlowOptions {
	client?: LiveClient;
	/** The checkout this Deckhand runs in: the repository is the one it belongs to. */
	cwd: string;
	/** Every session of the repository (the sidebar shows only this checkout's). */
	sessions: SessionRecord[];
	spinnerFrame: string;
	onExit: () => void;
	/** enter on a worktree: select its session. */
	onGoTo: (sessionId: string) => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
}

export interface WorktreesFlow {
	handleInput(input: string, key: Partial<Key>): void;
	render(width: number, height: number): React.ReactNode;
	hint(width: number): string;
	/** W: loads the list, the selection on `selectPath` (the selected session's worktree) when listed. */
	open(selectPath?: string): void;
}

export function useWorktreesFlow({client, cwd, sessions, spinnerFrame, onExit, onGoTo, setError, setStatusMessage}: WorktreesFlowOptions): WorktreesFlow {
	const [overview, setOverview] = useState<WorktreeOverview | undefined>();
	const [loading, setLoading] = useState(false);
	const [selected, setSelected] = useState<{key?: string; index: number}>({index: 0});
	const [confirm, setConfirm] = useState<DeleteConfirm | undefined>();
	const [working, setWorking] = useState<string | undefined>();
	const loadId = useRef(0);
	const checkId = useRef(0);
	const top = useRef(0);

	const now = Date.now();
	// Which sessions run comes from the live session list, not the (older) overview.
	const live = overview && {...overview, entries: overview.entries.map(entry => ({...entry, runningIds: entry.sessionIds.filter(id => {
		const session = sessions.find(item => item.id === id);
		return session !== undefined && session.status !== 'exited';
	})}))};
	const rows = worktreeRows(live, sessions, now);
	const selectable = rows.map((row, index) => (selectableWorktreeRow(row) ? index : -1)).filter(index => index >= 0);
	const keyedIndex = selected.key ? rows.findIndex(row => worktreeRowKey(row) === selected.key) : -1;
	const current = keyedIndex >= 0 ? keyedIndex : selectable.length ? selectable[Math.min(selected.index, selectable.length - 1)]! : -1;
	const currentRow = current >= 0 ? rows[current] : undefined;
	const currentEntry = currentRow?.kind === 'worktree' ? currentRow.entry : undefined;

	const select = (index: number) => setSelected({key: worktreeRowKey(rows[index]!), index: Math.max(0, selectable.indexOf(index))});
	const move = (direction: number) => {
		if (!selectable.length) return;
		const at = Math.max(0, selectable.indexOf(current));
		select(selectable[Math.min(selectable.length - 1, Math.max(0, at + direction))]!);
	};

	const load = (selectPath?: string) => {
		if (!client) { setError('still connecting to daemon'); return; }
		const id = ++loadId.current;
		setLoading(true);
		void client.worktreeOverview(cwd).then(next => {
			if (loadId.current !== id) return;
			setOverview(next);
			if (selectPath) setSelected({key: `wt:${selectPath}`, index: 0});
		}, error => { if (loadId.current === id) setError(errorMessage(error)); }).finally(() => { if (loadId.current === id) setLoading(false); });
	};

	const sessionsOf = (entry: WorktreeOverviewEntry) => sessions.filter(session => entry.sessionIds.includes(session.id));
	const runningOf = (entry: WorktreeOverviewEntry) => sessionsOf(entry).filter(session => session.status !== 'exited');
	const canDeleteBranch = (entry: WorktreeOverviewEntry) => Boolean(entry.branch) && !protectedBranch(entry.branch, overview?.defaultBranch);

	const openConfirm = (entries: WorktreeOverviewEntry[], bulk: boolean) => {
		const id = ++checkId.current;
		setConfirm({entries, bulk, index: 0, checks: {}});
		const entry = entries[0];
		if (bulk || !entry || entry.missing || !client) return;
		const check = (deleteBranch: boolean) => void client.inspectWorktree(cwd, entry.path, deleteBranch).then(
			result => result,
			error => ({safe: false, reasons: [errorMessage(error)], dirtyFiles: 0, untrackedFiles: 0, ignoredFiles: 0, structuralBlockers: [], running: []}) satisfies WorktreeCleanupInspection,
		).then(result => {
			if (checkId.current === id) setConfirm(state => (state ? {...state, checks: {...state.checks, [deleteBranch ? 'branch' : 'worktree']: result}} : state));
		});
		check(false);
		if (canDeleteBranch(entry)) check(true);
	};

	const options = (state: DeleteConfirm): ConfirmOption[] => {
		const cancel: ConfirmOption = {kind: 'cancel', label: 'Cancel'};
		const running = state.entries.reduce((count, entry) => count + runningOf(entry).length, 0);
		const stop = running ? `Stop ${running} session${running === 1 ? '' : 's'}, ` : '';
		const verb = (text: string) => (stop ? `${stop}${text}` : `${text[0]!.toUpperCase()}${text.slice(1)}`);
		if (state.bulk) {
			const count = state.entries.length, noun = `${count} worktree${count === 1 ? '' : 's'}`;
			return [{kind: 'delete-branch', label: verb(`delete ${noun} and their branches`)}, {kind: 'delete', label: verb(`delete ${noun}, keep the branches`)}, cancel];
		}
		const entry = state.entries[0]!;
		if (entry.missing === 'prunable') return [{kind: 'prune', label: 'Prune it: Git forgets the worktree (its branch stays)'}, cancel];
		if (entry.missing === 'unlisted') return [{kind: 'forget', label: "Forget it: Deckhand's record only, nothing on disk"}, cancel];
		const keep: ConfirmOption = {kind: 'delete', label: verb(entry.branch ? 'delete the worktree, keep its branch' : 'delete the worktree')};
		if (!canDeleteBranch(entry)) return [keep, cancel];
		const withBranch: ConfirmOption = {kind: 'delete-branch', label: verb(`delete the worktree and branch ${entry.branch}`)};
		// Merged work's branch has nothing left to keep: offered first.
		return mergedText(entry, overview?.defaultBranch) ? [withBranch, keep, cancel] : [keep, withBranch, cancel];
	};

	const finish = (message: string) => {
		setWorking(undefined);
		setConfirm(undefined);
		setStatusMessage(message);
		load();
	};

	const deleteOne = (entry: WorktreeOverviewEntry, deleteBranch: boolean, allowDataLoss = false) => {
		if (!client || working) return;
		setWorking(`Deleting ${worktreeName(entry)}…`);
		setError(undefined);
		void client.deleteWorktree(cwd, entry.path, {branch: entry.branch, deleteBranch, stopSessions: true, allowDataLoss}).then(result => {
			const done = result.removed === 'deleted' ? `Deleted ${worktreeName(entry)}${result.branchDeleted ? ' and its branch' : ''}` : result.removed === 'pruned' ? `Pruned ${worktreeName(entry)}` : `Forgot ${worktreeName(entry)}`;
			const extra = [result.stopped ? `stopped ${result.stopped} session${result.stopped === 1 ? '' : 's'}` : '', result.archived ? `archived ${result.archived} (f A shows them)` : ''].filter(Boolean);
			finish([done, ...extra].join(' · '));
		}, error => {
			setWorking(undefined);
			setConfirm(undefined);
			setError(errorMessage(error));
			load();
		});
	};

	const deleteAll = (entries: WorktreeOverviewEntry[], withBranches: boolean) => {
		if (!client || working) return;
		setError(undefined);
		void (async () => {
			let deleted = 0, branches = 0, stopped = 0;
			const failed: string[] = [];
			for (const [index, entry] of entries.entries()) {
				setWorking(`Deleting ${worktreeName(entry)}… ${index + 1}/${entries.length}`);
				try {
					const result = await client.deleteWorktree(cwd, entry.path, {branch: entry.branch, deleteBranch: withBranches && canDeleteBranch(entry), stopSessions: true});
					deleted++; stopped += result.stopped;
					if (result.branchDeleted) branches++;
				} catch (error) { failed.push(`${worktreeName(entry)}: ${errorMessage(error)}`); }
			}
			if (failed.length) setError(`Not deleted: ${failed.join('; ')}`);
			finish([`Deleted ${deleted} worktree${deleted === 1 ? '' : 's'}${branches ? ` and ${branches} branch${branches === 1 ? '' : 'es'}` : ''}`, stopped ? `stopped ${stopped} session${stopped === 1 ? '' : 's'}` : ''].filter(Boolean).join(' · '));
		})();
	};

	const confirmInput = (input: string, key: Partial<Key>, state: DeleteConfirm) => {
		if (working) return;
		const list = options(state);
		const index = Math.min(state.index, list.length - 1);
		if (state.override) {
			const {override} = state;
			if (key.escape) { setConfirm({...state, override: undefined}); return; }
			if (key.backspace || key.delete) { setConfirm({...state, override: {...override, draft: override.draft.slice(0, -1)}}); return; }
			if (key.return) { if (override.draft === 'DELETE') deleteOne(state.entries[0]!, override.deleteBranch, true); return; }
			if (!key.ctrl && !key.meta && /^[A-Za-z]+$/.test(input)) setConfirm({...state, override: {...override, draft: (override.draft + input).slice(0, 10)}});
			return;
		}
		if (key.escape) { setConfirm(undefined); return; }
		if (key.upArrow || input === 'k') { setConfirm({...state, index: (index - 1 + list.length) % list.length}); return; }
		if (key.downArrow || input === 'j') { setConfirm({...state, index: (index + 1) % list.length}); return; }
		if (!key.return) return;
		const option = list[index]!;
		if (option.kind === 'cancel') { setConfirm(undefined); return; }
		if (state.bulk) { deleteAll(state.entries, option.kind === 'delete-branch'); return; }
		const entry = state.entries[0]!;
		if (option.kind === 'prune' || option.kind === 'forget') { deleteOne(entry, false); return; }
		const deleteBranch = option.kind === 'delete-branch';
		const check = deleteBranch ? state.checks.branch : state.checks.worktree;
		if (!check) { setStatusMessage('Still checking what it would lose…'); return; }
		if (check.structuralBlockers.length) { setError(check.structuralBlockers.join('; ')); return; }
		if (check.safe) deleteOne(entry, deleteBranch);
		else setConfirm({...state, override: {deleteBranch, draft: ''}});
	};

	const handleInput = (input: string, key: Partial<Key>) => {
		if (confirm) { confirmInput(input, key, confirm); return; }
		if (key.escape) { onExit(); return; }
		if (key.upArrow || input === 'k') { move(-1); return; }
		if (key.downArrow || input === 'j') { move(1); return; }
		if (key.pageUp) { move(-10); return; }
		if (key.pageDown) { move(10); return; }
		if ((input === 'g' || key.home) && selectable.length) { select(selectable[0]!); return; }
		if ((input === 'G' || key.end) && selectable.length) { select(selectable.at(-1)!); return; }
		if (input === 'R') { load(); setStatusMessage('Checking the worktrees again'); return; }
		if (working) return;
		if (input === 'x') {
			if (currentRow?.kind === 'heading' && currentRow.group === 'ready') {
				const targets = bulkTargets(rows);
				if (targets.length) openConfirm(targets, true);
				else setStatusMessage('None of them can be deleted from here');
				return;
			}
			if (!currentEntry) { setStatusMessage('Select a worktree first (j/k)'); return; }
			const refusal = deleteRefusal(currentEntry);
			if (refusal) { setStatusMessage(refusal); return; }
			openConfirm([currentEntry], false);
			return;
		}
		if (!currentEntry) {
			if (key.return || input === 'o' || input === 'E' || input === 'M') setStatusMessage('Select a worktree first (j/k)');
			return;
		}
		if (key.return || input === 'o') {
			const lead = leadSession(sessionsOf(currentEntry));
			if (lead) onGoTo(lead.id);
			else setStatusMessage(currentEntry.isMain ? 'No session in the main checkout' : 'No session in it: x deletes it, E opens it in your editor');
			return;
		}
		if (input === 'E') {
			if (currentEntry.missing) { setStatusMessage('Its directory is gone'); return; }
			const label = openInEditor(currentEntry.path, setError);
			if (label) setStatusMessage(`Opened ${worktreeName(currentEntry)} in ${label}`);
			return;
		}
		if (input === 'M') {
			// The merged marker belongs to the worktree's record; any of its sessions sets or clears it.
			const owner = currentEntry.recordId ? sessionsOf(currentEntry).find(session => session.worktree?.id === currentEntry.recordId) : undefined;
			if (!owner || !client) { setStatusMessage(currentEntry.isMain ? 'The main checkout has nothing to merge' : 'Only worktrees Deckhand tracks have a merged marker'); return; }
			const wasMerged = Boolean(currentEntry.markers?.mergedAt);
			void client.markSessionMerged(owner.id, cwd).then(() => { setStatusMessage(wasMerged ? `Cleared the merged marker of ${worktreeName(currentEntry)}` : `Marked ${worktreeName(currentEntry)} merged`); load(); }, error => setError(errorMessage(error)));
			return;
		}
	};

	const render = (width: number, height: number): React.ReactNode => {
		if (confirm) return <ConfirmPane state={confirm} options={options(confirm)} sessions={sessions} working={working} width={width} height={height} />;
		const inner = Math.max(10, width - 4);
		const linked = overview?.entries.filter(entry => !entry.isMain) ?? [];
		const ready = rows.filter(row => row.kind === 'worktree' && row.group === 'ready').length;
		const right = overview ? `${linked.length} worktree${linked.length === 1 ? '' : 's'}${ready ? ` · ${ready} ready to delete` : ''}` : '';
		const status = working ?? (loading ? 'Checking worktrees…' : overview ? `${overview.defaultBranch ? `Compared with ${overview.defaultBranch}` : 'No default branch found'} · checked ${checkedAgo(now - Date.parse(overview.checkedAt))}${overview.unchecked ? ` · ${overview.unchecked} not checked (too many)` : ''}` : '');
		const lower = detailLines(currentRow, rows, sessions, overview?.defaultBranch, spinnerFrame, now, inner);
		const listRows = Math.max(1, height - 2 - 3 - (lower.length ? lower.length + 1 : 0));
		top.current = scrolledListTop(top.current, current, listRows, rows.length, index => selectableWorktreeRow(rows[index]!));
		const shown = rows.slice(top.current, top.current + listRows);
		return (
			<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
				<Box justifyContent="space-between" width={inner}>
					<Text wrap="truncate-end"><Text color={THEME.active} bold>⎇ Worktrees</Text><Text color={THEME.muted}> · {truncate(path.basename(overview?.mainRoot ?? cwd), Math.max(4, inner - right.length - 16))}</Text></Text>
					<Text color={THEME.muted}>{right}</Text>
				</Box>
				<Text color={working ? THEME.warn : THEME.muted} wrap="truncate-end">{status || ' '}</Text>
				<Box flexDirection="column" height={listRows}>
					{shown.map((row, index) => <WorktreeLine key={worktreeRowKey(row) ?? `row-${top.current + index}`} row={row} selected={top.current + index === current} width={inner} sessions={sessions} defaultBranch={overview?.defaultBranch} now={now} />)}
				</Box>
				{rows.length > top.current + listRows ? <Text color={THEME.muted}>{`  +${rows.length - top.current - listRows} more`}</Text> : <Text> </Text>}
				{lower.length ? <Text color={THEME.border}>{'─'.repeat(inner)}</Text> : null}
				{lower.map((line, index) => <React.Fragment key={`lower-${index}`}>{line}</React.Fragment>)}
			</Box>
		);
	};

	const hint = (width: number): string => {
		if (confirm?.override) return fitHint(['type DELETE then enter', 'esc back'], width, ' • ');
		if (confirm) return fitHint(['enter choose', 'j/k move', 'esc cancel'], width, ' • ');
		const onReady = currentRow?.kind === 'heading' && currentRow.group === 'ready';
		return fitHint([
			'j/k move',
			onReady ? {text: 'x delete them all', short: 'x delete all'} : 'x delete',
			{text: 'enter its session', short: 'enter session', drop: 1},
			{text: 'M merged', drop: 2},
			{text: 'E editor', drop: 3},
			{text: 'R refresh', drop: 2},
			'esc back',
		], width, ' • ');
	};

	return {
		handleInput,
		render,
		hint,
		open: (selectPath?: string) => {
			setConfirm(undefined);
			setSelected(selectPath ? {key: `wt:${selectPath}`, index: 0} : {index: 0});
			load(selectPath);
		},
	};
}

function checkedAgo(ms: number): string {
	const age = formatAge(ms);
	return age === 'now' ? 'just now' : `${age} ago`;
}

function WorktreeLine({row, selected, width, sessions, defaultBranch, now}: {row: WorktreeRow; selected: boolean; width: number; sessions: SessionRecord[]; defaultBranch?: string; now: number}) {
	if (row.kind === 'empty') return <Text color={THEME.muted} wrap="truncate-end">{`  ${row.text}`}</Text>;
	if (row.kind === 'heading') {
		const color = row.group === 'ready' ? THEME.success : row.group === 'leftovers' || row.group === 'missing' ? THEME.warn : THEME.muted;
		const text = `${selected ? '›' : ' '} ${GROUP_TITLES[row.group]} · ${row.count}`;
		return <Text inverse={selected} bold color={selected ? THEME.active : color} wrap="truncate-end">{selected ? `${text}  (x deletes them all)`.padEnd(width) : text}</Text>;
	}
	const {entry} = row;
	const merged = mergedText(entry, defaultBranch);
	const glyph = row.group === 'main' ? {text: '◆', color: THEME.muted} : entry.missing ? {text: '?', color: THEME.warn} : merged ? {text: '✓', color: row.group === 'ready' ? THEME.success : THEME.warn} : entry.runningIds.length ? {text: '●', color: THEME.success} : {text: '○', color: THEME.muted};
	const right = truncate(worktreeTags(entry, sessions, now).join(' · '), Math.floor(width / 2));
	const name = truncate(worktreeName(entry), Math.max(1, width - right.length - 6));
	return (
		<Text inverse={selected} bold={selected} color={selected ? THEME.active : undefined} wrap="truncate-end">
			{`${selected ? '›' : ' '} `}<Text color={selected ? undefined : glyph.color}>{glyph.text}</Text>{` ${name}`.padEnd(Math.max(0, width - right.length - 3))}<Text color={selected ? undefined : THEME.muted}>{right}</Text>
		</Text>
	);
}

/** The selected worktree: name and merge state, path, sessions, what deleting it would lose; or what x on the ready heading deletes. */
function detailLines(row: WorktreeRow | undefined, rows: WorktreeRow[], sessions: SessionRecord[], defaultBranch: string | undefined, spinnerFrame: string, now: number, width: number): React.ReactNode[] {
	if (row?.kind === 'heading' && row.group === 'ready') {
		const targets = bulkTargets(rows);
		return [
			<Text key="t" bold wrap="truncate-end">{`x deletes ${targets.length === 1 ? 'this worktree' : `these ${targets.length} worktrees`} and their branches (after one confirmation)`}</Text>,
			<Text key="n" color={THEME.muted} wrap="truncate-end">{truncate(targets.map(worktreeName).join(', '), width * 2)}</Text>,
			<Text key="w" color={THEME.muted} wrap="truncate-end">Merged and clean: nothing is lost. Sessions running in them are stopped first.</Text>,
		];
	}
	if (row?.kind !== 'worktree') return [];
	const {entry} = row;
	const inWorktree = sessions.filter(session => entry.sessionIds.includes(session.id));
	const merged = mergedText(entry, defaultBranch);
	const state = row.group === 'main' ? 'the main checkout' : merged ?? (entry.missing ? 'its directory is gone' : row.group === 'idle' ? 'not merged, idle' : 'not merged');
	const inspection = entry.inspection;
	const loss = entry.isMain || entry.missing ? undefined
		: entry.error ? `Could not check it: ${entry.error}`
		: !inspection ? 'Not checked'
		: inspection.safe ? 'Clean: deleting it and its branch loses nothing'
		: `Deleting it would lose: ${inspection.reasons.join('; ')}`;
	const notes = [
		entry.inUse === 'this' ? 'this Deckhand runs here' : entry.inUse === 'other' ? 'another Deckhand is open here' : '',
		entry.locked ? 'locked' : '',
		entry.lastCommitAt ? `last commit ${formatAge(now - Date.parse(entry.lastCommitAt))} ago` : '',
		entry.aheadOfDefault !== undefined && !entry.isMain ? `${entry.aheadOfDefault} commit${entry.aheadOfDefault === 1 ? '' : 's'} not in ${defaultBranch ?? 'the default branch'}` : '',
	].filter(Boolean);
	return [
		<Text key="t" wrap="truncate-end"><Text bold>{worktreeName(entry)}</Text><Text color={merged ? THEME.success : THEME.muted}>{` · ${state}`}</Text></Text>,
		<Text key="p" color={THEME.muted} wrap="truncate-end">{compactPath(entry.path, width)}{notes.length ? ` · ${notes.join(' · ')}` : ''}</Text>,
		inWorktree.length
			? <Text key="s" wrap="truncate-end">{inWorktree.slice(0, 6).map((session, index) => <Text key={session.id}>{index ? ', ' : ''}<Text color={statusColor(session)}>{statusGlyph(session, spinnerFrame)}</Text>{` ${displaySessionTitle(session, sessions)}`}</Text>)}{inWorktree.length > 6 ? <Text color={THEME.muted}>{` +${inWorktree.length - 6}`}</Text> : null}</Text>
			: <Text key="s" color={THEME.muted} wrap="truncate-end">{entry.recordId ? 'No sessions left in it' : entry.isMain ? 'No sessions' : 'Not created or attached by Deckhand'}</Text>,
		...loss ? [<Text key="l" color={inspection?.safe ? THEME.success : THEME.warn} wrap="truncate-end">{loss}</Text>] : [],
	].slice(0, DETAIL_ROWS);
}

function ConfirmPane({state, options, sessions, working, width, height}: {state: DeleteConfirm; options: ConfirmOption[]; sessions: SessionRecord[]; working?: string; width: number; height: number}) {
	const inner = Math.max(1, width - 4);
	const index = Math.min(state.index, options.length - 1);
	const running = sessions.filter(session => session.status !== 'exited' && state.entries.some(entry => entry.sessionIds.includes(session.id)));
	const entry = state.entries[0]!;
	if (state.override) {
		const check = state.override.deleteBranch ? state.checks.branch : state.checks.worktree;
		return (
			<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderDanger} paddingX={1}>
				<Text color={THEME.error} bold>{`Delete ${worktreeName(entry)} anyway?`}</Text>
				<Text wrap="wrap">This permanently erases what is listed below. Main/current/shared worktree protections cannot be overridden.</Text>
				<Box marginTop={1} flexDirection="column">{check ? cleanupLossLines(check, inner, fileRowsLeft(height, 9 + check.reasons.length)) : <Text color={THEME.warn}>Safety could not be verified</Text>}</Box>
				<Box marginTop={1}><Text color={THEME.warn}>{`Type DELETE then enter: ${state.override.draft}`}</Text></Box>
			</Box>
		);
	}
	const check = state.bulk ? undefined : options[index]?.kind === 'delete-branch' ? state.checks.branch : state.checks.worktree;
	const summary = state.bulk || entry.missing ? undefined
		: !check ? 'Checking what it would lose…'
		: check.structuralBlockers.length ? 'Deletion is blocked:'
		: check.safe ? 'Local cleanup checks passed: nothing is lost'
		: 'Deleting it would lose (typing DELETE overrides):';
	// What it would lose, in full: the reasons, then the files in the rows the rest of the pane leaves.
	const loss = check && !check.safe && !check.structuralBlockers.length
		? cleanupLossLines(check, inner, fileRowsLeft(height, 12 + check.reasons.length + options.length + (running.length ? 1 : 0))) : [];
	return (
		<Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderDanger} paddingX={1}>
			<Text color={THEME.error} bold>{state.bulk ? `Delete ${state.entries.length} merged worktree${state.entries.length === 1 ? '' : 's'}?` : entry.missing ? `Clean up ${worktreeName(entry)}?` : `Delete ${worktreeName(entry)}?`}</Text>
			{state.bulk
				? state.entries.slice(0, 8).map(item => <Text key={item.path} color={THEME.muted} wrap="truncate-end">{`  ${worktreeName(item)} · ${compactPath(item.path, Math.max(8, inner - worktreeName(item).length - 5))}`}</Text>)
				: <Text color={THEME.muted} wrap="truncate-end">{compactPath(entry.path, inner)}</Text>}
			{state.bulk && state.entries.length > 8 ? <Text color={THEME.muted}>{`  +${state.entries.length - 8} more`}</Text> : null}
			{summary ? <Text color={check?.safe ? THEME.success : THEME.warn} wrap="truncate-end">{summary}</Text> : null}
			{loss}
			{check?.structuralBlockers.map((blocker, row) => <Text key={`b-${row}`} color={THEME.warn} wrap="truncate-end">{`  ${blocker}`}</Text>)}
			{running.length ? <Text color={THEME.warn} wrap="truncate-end">{`Stops ${running.length === 1 ? 'the session' : `${running.length} sessions`} running there first: ${running.map(session => displaySessionTitle(session, sessions)).join(', ')}`}</Text> : null}
			{entry.missing ? null : <Text color={THEME.muted} wrap="truncate-end">Its sessions cannot be resumed afterwards; they are archived (notes kept, f A shows them).</Text>}
			<Box marginTop={1} flexDirection="column">
				{options.map((option, row) => <SelectableRow key={option.kind} selected={row === index} text={option.label} width={inner} selectedColor={option.kind === 'cancel' ? THEME.muted : THEME.error} wrap />)}
			</Box>
			{working ? <Box marginTop={1}><Text color={THEME.warn}>{working}</Text></Box> : null}
		</Box>
	);
}
